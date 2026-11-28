//! Read-only snapshots, design §8 "Snapshots": a commit's tree written from the store's blobs,
//! with no `.git`, files 0444 (0555 when executable) and directories 0555, and the registry that
//! shares one snapshot per commit among the jobs on it.
//!
//! The tree is written by the adapter itself, from `list_tree` and one `git cat-file --batch`, so
//! its bytes are the blobs' bytes: no attributes, filters or umask apply. Every path component is
//! checked, nothing is written through a symlink, and the tree is written into a temporary
//! sibling of its destination that is renamed into place.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{DirBuilder, File, OpenOptions, Permissions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use mintswarm_domain::ids::RevId;
use mintswarm_domain::vcs::{TreeEntry, TreeMode, Workspace};

use super::env::failure;
use super::{GitError, GitStore, read, store_path};

/// The mode of a regular file in a snapshot.
const FILE_MODE: u32 = 0o444;
/// The mode of an executable file and of every directory in a snapshot.
const EXEC_MODE: u32 = 0o555;
/// The mode of a directory while it is written, and of the snapshots root.
const PRIVATE_MODE: u32 = 0o700;
/// The prefix of a snapshot's temporary directory.
const TEMPORARY: &str = ".tmp-";

/// The type bits of a tree entry's mode.
const TYPE_MASK: u32 = 0o170_000;
const REGULAR: u32 = 0o100_000;
const SYMLINK: u32 = 0o120_000;
const GITLINK: u32 = 0o160_000;

/// Temporaries made by this process, so two never share a name.
static TEMPORARIES: AtomicU64 = AtomicU64::new(0);

/// An I/O failure, in our own words.
fn io(context: String) -> impl FnOnce(std::io::Error) -> GitError {
    move |source| GitError::Io { context, source }
}

/// A path for messages.
fn shown(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

/// The components of a tree path, refused when one is empty, `.`, `..` or `.git` in any case.
fn components(path: &[u8]) -> Result<Vec<&[u8]>, GitError> {
    let parts: Vec<&[u8]> = path.split(|&byte| byte == b'/').collect();
    for part in &parts {
        if part.is_empty()
            || *part == b"."
            || *part == b".."
            || part.eq_ignore_ascii_case(b".git")
            || part.contains(&0)
        {
            return Err(GitError::UnsafePath(shown(path)));
        }
    }
    Ok(parts)
}

/// The mode a regular file gets for a blob of `mode`.
fn file_mode(mode: u32) -> u32 {
    if mode & 0o111 != 0 {
        EXEC_MODE
    } else {
        FILE_MODE
    }
}

/// One `git cat-file --batch`, asked for one object at a time.
pub(super) struct Batch {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Batch {
    pub(super) fn start(store: &GitStore) -> Result<Batch, GitError> {
        let mut command = store.command(["cat-file", "--batch"]);
        command.stdin(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(io("cannot run git cat-file".to_owned()))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitError::Parse {
                command: "cat-file".to_owned(),
                detail: "no pipes to the process".to_owned(),
            });
        };
        Ok(Batch {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    /// The bytes of the blob `id`.
    fn blob(&mut self, id: &str) -> Result<Vec<u8>, GitError> {
        self.object(id, "blob")
    }

    /// The bytes of the object `id`, which must be of type `kind`, such as `blob` or `commit`.
    pub(super) fn object(&mut self, id: &str, kind: &str) -> Result<Vec<u8>, GitError> {
        let parse = |detail: String| GitError::Parse {
            command: "cat-file".to_owned(),
            detail,
        };
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| parse("the request pipe is closed".to_owned()))?;
        stdin
            .write_all(format!("{id}\n").as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(io("cannot write to git cat-file".to_owned()))?;
        let mut header = Vec::new();
        self.stdout
            .read_until(b'\n', &mut header)
            .map_err(io("cannot read from git cat-file".to_owned()))?;
        let header = String::from_utf8_lossy(&header).into_owned();
        let mut fields = header.trim_end().split(' ');
        let size = match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some(got), Some(got_kind), Some(size), None) if got == id && got_kind == kind => size
                .parse::<usize>()
                .map_err(|_| parse(format!("header {header:?}")))?,
            _ => return Err(parse(format!("header {header:?} for {id}"))),
        };
        let mut data = vec![0; size + 1];
        self.stdout
            .read_exact(&mut data)
            .map_err(io("cannot read from git cat-file".to_owned()))?;
        if data.pop() != Some(b'\n') {
            return Err(parse(format!("{kind} {id} without its final newline")));
        }
        Ok(data)
    }

    /// Close the requests and wait for the process, which must succeed.
    pub(super) fn finish(mut self) -> Result<(), GitError> {
        drop(self.stdin.take());
        let mut rest = Vec::new();
        self.stdout
            .read_to_end(&mut rest)
            .map_err(io("cannot read from git cat-file".to_owned()))?;
        let mut stderr = Vec::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            pipe.read_to_end(&mut stderr)
                .map_err(io("cannot read from git cat-file".to_owned()))?;
        }
        let status = self
            .child
            .wait()
            .map_err(io("cannot wait for git cat-file".to_owned()))?;
        if !status.success() {
            let output = Output {
                status,
                stdout: Vec::new(),
                stderr,
            };
            return Err(failure("cat-file", &output));
        }
        if !rest.is_empty() {
            return Err(GitError::Parse {
                command: "cat-file".to_owned(),
                detail: "output after the last object".to_owned(),
            });
        }
        Ok(())
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            drop(self.stdin.take());
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Write `rev`'s tree read-only at `dest`, which must not exist; design §8 "Snapshots".
pub(super) fn materialise(
    store: &GitStore,
    rev: &RevId,
    dest: &Path,
    mode: TreeMode,
) -> Result<Workspace, GitError> {
    if mode != TreeMode::ReadOnly {
        return Err(GitError::Unsupported("materialise"));
    }
    read::commit(store, rev)?;
    match std::fs::symlink_metadata(dest) {
        Ok(_) => return Err(GitError::Exists(dest.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(GitError::Io {
                context: format!("cannot read {}", dest.display()),
                source,
            });
        }
    }
    let entries = super::Vcs::list_tree(store, rev)?;
    for entry in &entries {
        components(&entry.path.0)?;
    }
    let (Some(parent), Some(name)) = (dest.parent(), dest.file_name()) else {
        return Err(GitError::UnsafePath(dest.display().to_string()));
    };
    let mut temporary = OsStr::new(TEMPORARY).to_owned();
    temporary.push(name);
    temporary.push(format!(
        "-{}-{}",
        std::process::id(),
        TEMPORARIES.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = parent.join(temporary);
    DirBuilder::new()
        .mode(PRIVATE_MODE)
        .create(&temporary)
        .map_err(io(format!("cannot create {}", temporary.display())))?;
    if let Err(error) = write_tree(store, &entries, &temporary) {
        let _ = remove_tree(&temporary);
        return Err(error);
    }
    if let Err(source) = std::fs::rename(&temporary, dest) {
        let _ = remove_tree(&temporary);
        return Err(GitError::Io {
            context: format!(
                "cannot rename {} to {}",
                temporary.display(),
                dest.display()
            ),
            source,
        });
    }
    if let Err(source) = std::fs::set_permissions(dest, Permissions::from_mode(EXEC_MODE)) {
        let _ = remove_tree(dest);
        return Err(GitError::Io {
            context: format!("cannot set the mode of {}", dest.display()),
            source,
        });
    }
    Ok(Workspace {
        path: dest.to_owned(),
        parent: rev.clone(),
        mode: TreeMode::ReadOnly,
    })
}

/// Write `entries` under `root`, then set every directory under it to 0555, leaves first.
fn write_tree(store: &GitStore, entries: &[TreeEntry], root: &Path) -> Result<(), GitError> {
    let mut batch = Batch::start(store)?;
    let mut directories: Vec<PathBuf> = Vec::new();
    for entry in entries {
        write_entry(&mut batch, entry, root, &mut directories)?;
    }
    batch.finish()?;
    directories.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in &directories {
        std::fs::set_permissions(dir, Permissions::from_mode(EXEC_MODE))
            .map_err(io(format!("cannot set the mode of {}", dir.display())))?;
    }
    Ok(())
}

/// Write one entry under `root`, creating its directories one component at a time.
fn write_entry(
    batch: &mut Batch,
    entry: &TreeEntry,
    root: &Path,
    directories: &mut Vec<PathBuf>,
) -> Result<(), GitError> {
    let unsafe_path = || GitError::UnsafePath(shown(&entry.path.0));
    let parts = components(&entry.path.0)?;
    let Some((leaf, dirs)) = parts.split_last() else {
        return Err(unsafe_path());
    };
    let mut path = root.to_owned();
    for dir in dirs {
        path.push(OsStr::from_bytes(dir));
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(unsafe_path()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_dir(&path, &entry.path.0)?;
                directories.push(path.clone());
            }
            Err(source) => {
                return Err(GitError::Io {
                    context: format!("cannot read {}", path.display()),
                    source,
                });
            }
        }
    }
    path.push(OsStr::from_bytes(leaf));
    let exists = |error: &std::io::Error| error.kind() == std::io::ErrorKind::AlreadyExists;
    match entry.mode & TYPE_MASK {
        REGULAR => {
            let data = batch.blob(&entry.blob.0)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|error| {
                    if exists(&error) {
                        unsafe_path()
                    } else {
                        GitError::Io {
                            context: format!("cannot create {}", path.display()),
                            source: error,
                        }
                    }
                })?;
            write_file(&mut file, &data, file_mode(entry.mode), &path)
        }
        SYMLINK => {
            let target = batch.blob(&entry.blob.0)?;
            if target.is_empty() || target.contains(&0) {
                return Err(unsafe_path());
            }
            std::os::unix::fs::symlink(OsStr::from_bytes(&target), &path).map_err(|error| {
                if exists(&error) {
                    unsafe_path()
                } else {
                    GitError::Io {
                        context: format!("cannot create the link {}", path.display()),
                        source: error,
                    }
                }
            })
        }
        GITLINK => {
            create_dir(&path, &entry.path.0)?;
            directories.push(path);
            Ok(())
        }
        _ => Err(GitError::Parse {
            command: "ls-tree".to_owned(),
            detail: format!("mode {:o} at {}", entry.mode, shown(&entry.path.0)),
        }),
    }
}

/// Create one directory, writable while the tree is written; an existing entry is refused.
fn create_dir(path: &Path, entry: &[u8]) -> Result<(), GitError> {
    DirBuilder::new()
        .mode(PRIVATE_MODE)
        .create(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                GitError::UnsafePath(shown(entry))
            } else {
                GitError::Io {
                    context: format!("cannot create {}", path.display()),
                    source: error,
                }
            }
        })
}

/// Write a file's bytes and set its mode exactly, whatever the umask.
fn write_file(file: &mut File, data: &[u8], mode: u32, path: &Path) -> Result<(), GitError> {
    file.write_all(data)
        .map_err(io(format!("cannot write {}", path.display())))?;
    file.set_permissions(Permissions::from_mode(mode))
        .map_err(io(format!("cannot set the mode of {}", path.display())))
}

/// Remove `path` and everything under it, making each directory writable first and never
/// following a symlink. A missing path is not an error.
pub(super) fn remove_tree(path: &Path) -> Result<(), GitError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(GitError::Io {
                context: format!("cannot read {}", path.display()),
                source,
            });
        }
    };
    if !meta.is_dir() {
        return std::fs::remove_file(path).map_err(io(format!("cannot remove {}", path.display())));
    }
    std::fs::set_permissions(path, Permissions::from_mode(PRIVATE_MODE))
        .map_err(io(format!("cannot set the mode of {}", path.display())))?;
    let children =
        std::fs::read_dir(path).map_err(io(format!("cannot read {}", path.display())))?;
    for child in children {
        let child = child.map_err(io(format!("cannot read {}", path.display())))?;
        remove_tree(&child.path())?;
    }
    std::fs::remove_dir(path).map_err(io(format!("cannot remove {}", path.display())))
}

/// The snapshots of `repository` under the runner's state directory:
/// `<state>/snapshots/<repository>`.
///
/// The name must be one path component that does not start with `.`, as for [`store_path`].
pub fn snapshots_path(runner_dir: &Path, repository: &str) -> Result<PathBuf, GitError> {
    store_path(runner_dir, repository)?;
    Ok(runner_dir
        .join(crate::exec::store::SNAPSHOTS_DIR)
        .join(repository))
}

/// One snapshot the registry holds.
#[derive(Debug)]
struct Held {
    path: PathBuf,
    count: usize,
}

/// What the registry's lock guards.
#[derive(Debug, Default)]
struct State {
    held: HashMap<String, Held>,
    base_head: Option<String>,
}

/// The read-only snapshots of one repository: one per commit, shared by the jobs on it, and
/// evicted when no job holds it and it is not the base head.
///
/// The counts are kept in memory: [`Snapshots::open`] removes every snapshot a previous run left,
/// and jobs acquire theirs again.
#[derive(Debug)]
pub struct Snapshots {
    root: PathBuf,
    /// One lock serialises acquire, release and materialisation, so a second job on a commit
    /// waits for the first to write its snapshot and then shares it.
    state: Mutex<State>,
}

impl Snapshots {
    /// The registry over `root`, created with mode 0700 when missing; everything already under it
    /// is removed.
    pub fn open(root: &Path) -> Result<Snapshots, GitError> {
        DirBuilder::new()
            .recursive(true)
            .mode(PRIVATE_MODE)
            .create(root)
            .map_err(io(format!("cannot create {}", root.display())))?;
        let meta = std::fs::symlink_metadata(root)
            .map_err(io(format!("cannot read {}", root.display())))?;
        if !meta.is_dir() {
            return Err(GitError::UnsafePath(root.display().to_string()));
        }
        std::fs::set_permissions(root, Permissions::from_mode(PRIVATE_MODE))
            .map_err(io(format!("cannot set the mode of {}", root.display())))?;
        let entries =
            std::fs::read_dir(root).map_err(io(format!("cannot read {}", root.display())))?;
        for entry in entries {
            let entry = entry.map_err(io(format!("cannot read {}", root.display())))?;
            remove_tree(&entry.path())?;
        }
        Ok(Snapshots {
            root: root.to_owned(),
            state: Mutex::new(State::default()),
        })
    }

    /// The snapshots' directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The snapshot of `rev` for one more job: written from `store` on first use, and shared
    /// afterwards.
    pub fn acquire(&self, store: &GitStore, rev: &RevId) -> Result<Workspace, GitError> {
        let id = read::commit(store, rev)?.to_owned();
        let mut state = self.state();
        let path = match state.held.get_mut(&id) {
            Some(held) => {
                held.count += 1;
                held.path.clone()
            }
            None => {
                let path = self.root.join(&id);
                // Not held, so whatever is there is left over from a failed eviction.
                remove_tree(&path)?;
                materialise(store, rev, &path, TreeMode::ReadOnly)?;
                state.held.insert(
                    id,
                    Held {
                        path: path.clone(),
                        count: 1,
                    },
                );
                path
            }
        };
        Ok(Workspace {
            path,
            parent: rev.clone(),
            mode: TreeMode::ReadOnly,
        })
    }

    /// One job is done with the snapshot of `rev`; at a count of 0 it is evicted unless it is the
    /// base head.
    pub fn release(&self, rev: &RevId) -> Result<(), GitError> {
        let mut state = self.state();
        let is_head = state.base_head.as_deref() == Some(rev.0.as_str());
        let held = match state.held.get_mut(&rev.0) {
            Some(held) if held.count > 0 => held,
            _ => return Err(GitError::NotHeld(rev.0.clone())),
        };
        held.count -= 1;
        if held.count == 0 && !is_head {
            evict(&mut state, &rev.0)?;
        }
        Ok(())
    }

    /// The repository's base head is now `rev`; the previous head's snapshot is evicted when no
    /// job holds it.
    pub fn set_base_head(&self, rev: &RevId) -> Result<(), GitError> {
        let mut state = self.state();
        let previous = state.base_head.replace(rev.0.clone());
        match previous {
            Some(previous) if previous != rev.0 => {
                if state
                    .held
                    .get(&previous)
                    .is_some_and(|held| held.count == 0)
                {
                    evict(&mut state, &previous)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// How many jobs hold the snapshot of `rev`.
    pub fn count(&self, rev: &RevId) -> usize {
        self.state().held.get(&rev.0).map_or(0, |held| held.count)
    }
}

/// Forget the snapshot of `id` and remove its tree.
fn evict(state: &mut State, id: &str) -> Result<(), GitError> {
    match state.held.remove(id) {
        Some(held) => remove_tree(&held.path),
        None => Ok(()),
    }
}
