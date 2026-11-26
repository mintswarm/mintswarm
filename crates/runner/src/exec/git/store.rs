//! Creating and opening the trusted store: mode 0700, bare, neither shallow nor partial, no
//! alternates and no attributes of its own, and a git that meets [`GIT_REQUIREMENT`]; and opening
//! a working directory's repository, which is not trusted, for `mintswarm-runner tool msw-vcs`.

use std::ffi::OsStr;
use std::fs::{DirBuilder, Permissions};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use mintswarm_domain::ids::RevId;
use mintswarm_domain::probe::{Check, ProbeResult, ProbeValue, VersionRequirement};
use mintswarm_domain::retry::RetryPolicy;

use super::env::success;
use super::read::id_text;
use super::{GitError, GitStore};

/// The git versions the store works with, checked against the `tool.git` probe result.
pub const GIT_REQUIREMENT: &str = ">= 2.45";

/// The probe whose result [`GitStore::open`] takes.
const GIT_PROBE: &str = "tool.git";

/// The mode of the store's directory.
const STORE_MODE: u32 = 0o700;

/// Words git reads as false in a boolean setting.
const FALSE_WORDS: &[&str] = &["false", "no", "off", "0"];

/// A working directory's repository, opened by [`GitStore::open_checkout`].
#[derive(Debug)]
pub struct Checkout {
    /// The repository, on its absolute git directory.
    pub store: GitStore,
    /// The top of the working tree.
    pub top: PathBuf,
    /// The commit `HEAD` names, or `None` when it names none.
    pub head: Option<RevId>,
}

/// The store of `repository` under the runner's state directory: `<state>/repos/<repository>.git`.
///
/// The name must be one path component that does not start with `.`.
pub fn store_path(state_dir: &Path, repository: &str) -> Result<PathBuf, GitError> {
    if repository.is_empty() || repository.starts_with('.') || repository.contains(['/', '\0']) {
        return Err(GitError::StoreName(repository.to_owned()));
    }
    Ok(state_dir.join("repos").join(format!("{repository}.git")))
}

impl GitStore {
    /// Create an empty store at `path`, which must not exist, and open it.
    pub fn create(path: &Path, git: &ProbeResult) -> Result<GitStore, GitError> {
        check_version(git)?;
        let path = absolute(path)?;
        let io = |context: &str| {
            let context = format!("{context} {}", path.display());
            move |source| GitError::Io { context, source }
        };
        if let Some(parent) = path.parent() {
            DirBuilder::new()
                .recursive(true)
                .mode(STORE_MODE)
                .create(parent)
                .map_err(io("cannot create the parent of"))?;
        }
        DirBuilder::new()
            .mode(STORE_MODE)
            .create(&path)
            .map_err(io("cannot create the store"))?;
        // The umask may have narrowed the mode, never widened it; set it exactly.
        std::fs::set_permissions(&path, Permissions::from_mode(STORE_MODE))
            .map_err(io("cannot set the mode of"))?;
        GitStore::unchecked(path.clone()).git(["init", "--bare", "--quiet", "--template="])?;
        std::fs::set_permissions(&path, Permissions::from_mode(STORE_MODE))
            .map_err(io("cannot set the mode of"))?;
        GitStore::open(&path, git)
    }

    /// Open the store at `path`, refusing it unless it is fit to be trusted.
    pub fn open(path: &Path, git: &ProbeResult) -> Result<GitStore, GitError> {
        let warnings = check_version(git)?;
        for warning in &warnings {
            tracing::warn!(store = %path.display(), "{warning}");
        }
        let path = absolute(path)?;
        let meta = std::fs::symlink_metadata(&path).map_err(|source| GitError::Io {
            context: format!("cannot read {}", path.display()),
            source,
        })?;
        if !meta.is_dir() {
            return Err(GitError::NotBare(path));
        }
        let mode = meta.permissions().mode() & 0o7777;
        if mode != STORE_MODE {
            return Err(GitError::Mode { path, mode });
        }
        let mut store = GitStore::unchecked(path);
        store.check_shape()?;
        store.set_format()?;
        store.warnings = warnings;
        Ok(store)
    }

    /// Open the repository whose working tree holds `dir`, which is not trusted: none of
    /// [`GitStore::open`]'s checks are made. Its own attributes file, `info/attributes` in the
    /// git directory, is checked at each `diff` instead, which fails with
    /// [`GitError::Attributes`] while that file is not empty. A directory outside any working
    /// tree is [`GitError::NotInRepository`].
    pub fn open_checkout(dir: &Path) -> Result<Checkout, GitError> {
        let probe = GitStore::unchecked(PathBuf::new());
        // Discovered as git discovers it from `cwd`, never from a `GIT_DIR` of the environment.
        let discover = |cwd: &Path, args: &[&str]| {
            let mut command = probe.command(args);
            command.env_remove("GIT_DIR").current_dir(cwd);
            probe.output(command, "rev-parse")
        };
        let output = discover(dir, &["rev-parse", "--show-toplevel"])?;
        let top = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
        if !output.status.success() || top.is_empty() {
            return Err(GitError::NotInRepository(dir.to_owned()));
        }
        let top = PathBuf::from(OsStr::from_bytes(top));
        let path_of = |args: &[&str]| -> Result<PathBuf, GitError> {
            let out = success("rev-parse", discover(&top, args)?)?;
            let path = Path::new(OsStr::from_bytes(out.strip_suffix(b"\n").unwrap_or(&out)));
            Ok(top.join(path))
        };
        let git_dir = path_of(&["rev-parse", "--absolute-git-dir"])?;
        let attributes = path_of(&["rev-parse", "--git-path", "info/attributes"])?;
        let mut store = GitStore::unchecked(git_dir);
        store.set_format()?;
        store.attributes = Some(attributes);
        let output = store.output(
            store.command(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]),
            "rev-parse",
        )?;
        let head = match output.status.code() {
            Some(0) => Some(RevId(id_text(
                &store,
                output.stdout.trim_ascii_end(),
                "rev-parse",
            )?)),
            _ => None,
        };
        Ok(Checkout { store, top, head })
    }

    /// Read the object format, and set the empty tree as the attribute source.
    fn set_format(&mut self) -> Result<(), GitError> {
        self.id_len = match self.line(["rev-parse", "--show-object-format"])?.as_str() {
            "sha1" => 40,
            "sha256" => 64,
            other => {
                return Err(GitError::Parse {
                    command: "rev-parse".to_owned(),
                    detail: format!("unknown object format `{other}`"),
                });
            }
        };
        // The empty tree: no `.gitattributes` from any tree applies.
        self.attr_source = Some(self.line(["hash-object", "-t", "tree", "--stdin"])?);
        Ok(())
    }

    /// A store at `path` before any check.
    fn unchecked(path: PathBuf) -> GitStore {
        GitStore {
            path,
            program: PathBuf::from("git"),
            attr_source: None,
            id_len: 40,
            warnings: Vec::new(),
            allow_file: false,
            attributes: None,
            retry: RetryPolicy::default(),
        }
    }

    /// Refuse a store that is not bare, is shallow or partial, or has alternates or attributes.
    fn check_shape(&self) -> Result<(), GitError> {
        let path = || self.path.clone();
        let bare = self.output(
            self.command(["rev-parse", "--is-bare-repository"]),
            "rev-parse",
        )?;
        if !bare.status.success() || bare.stdout != b"true\n" {
            return Err(GitError::NotBare(path()));
        }
        if self.line(["rev-parse", "--is-shallow-repository"])? != "false" {
            return Err(GitError::Shallow(path()));
        }
        if let Some(key) = self.promisor_key()? {
            return Err(GitError::Partial { path: path(), key });
        }
        if non_empty(&self.path.join("objects/info/alternates"))? {
            return Err(GitError::Alternates(path()));
        }
        if non_empty(&self.path.join("info/attributes"))? {
            return Err(GitError::Attributes(path()));
        }
        Ok(())
    }

    /// The first setting that makes the store a partial clone, as `landing/floor.py` reads them.
    fn promisor_key(&self) -> Result<Option<String>, GitError> {
        let out = self.git(["config", "--list", "-z"])?;
        for entry in out.split(|&byte| byte == 0) {
            let entry = String::from_utf8_lossy(entry);
            let (key, value) = match entry.split_once('\n') {
                Some((key, value)) => (key, Some(value)),
                None => (entry.as_ref(), None),
            };
            let remote = |suffix: &str| key.starts_with("remote.") && key.ends_with(suffix);
            let truthy =
                value.is_none_or(|value| !FALSE_WORDS.contains(&value.to_lowercase().as_str()));
            if key == "extensions.partialclone"
                || (remote(".promisor") && truthy)
                || remote(".partialclonefilter")
            {
                return Ok(Some(key.to_owned()));
            }
        }
        Ok(None)
    }

    /// The first line of a git command's output, which must succeed.
    fn line<const N: usize>(&self, args: [&str; N]) -> Result<String, GitError> {
        let out = self.git(args)?;
        Ok(String::from_utf8_lossy(&out).trim_end().to_owned())
    }

    /// Set how `fetch` retries a transient failure; [`RetryPolicy::default`] until set.
    ///
    /// A network failure gets the policy's `attempts`; an unrecognised failure, or a fetch
    /// stopped by a signal, gets at most two. The policy's `total` bounds only the waits: a
    /// wait that would end after it is not started, and a running fetch is never cut off. A
    /// fetch that gives up on an unrecognised failure logs a warning, and its
    /// [`GitError::FetchExhausted`] carries a redacted record for the caller to keep in the
    /// runner's store.
    pub fn set_retry_policy(&mut self, policy: RetryPolicy) {
        self.retry = policy;
    }

    /// Allow `file://` remotes: the test-only remote.
    #[cfg(test)]
    pub(crate) fn allow_file_remote(&mut self) {
        self.allow_file = true;
    }

    /// Run `program` in place of git.
    #[cfg(test)]
    pub(super) fn set_program(&mut self, program: &Path) {
        self.program = program.to_owned();
    }
}

/// Check a `tool.git` probe result against [`GIT_REQUIREMENT`]; the warnings to give.
fn check_version(git: &ProbeResult) -> Result<Vec<String>, GitError> {
    let refuse = |found: String| GitError::Version {
        requirement: GIT_REQUIREMENT.to_owned(),
        found,
    };
    if git.namespace.to_string() != GIT_PROBE {
        return Err(refuse(format!("(a `{}` probe result)", git.namespace)));
    }
    let requirement =
        VersionRequirement::parse(GIT_REQUIREMENT).map_err(|error| refuse(error.to_string()))?;
    match requirement.check(&git.value) {
        Check::Satisfied => Ok(Vec::new()),
        Check::Unsatisfied => Err(refuse(match &git.value {
            ProbeValue::Known {
                version: Some(version),
                ..
            } => version.clone(),
            _ => "of unknown version".to_owned(),
        })),
        Check::Unknown => Ok(vec![format!(
            "the git version is unknown; the store needs git {GIT_REQUIREMENT}"
        )]),
    }
}

/// Whether a file exists with content; anything but a regular file counts as content.
pub(super) fn non_empty(path: &Path) -> Result<bool, GitError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(!meta.is_file() || meta.len() > 0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(GitError::Io {
            context: format!("cannot read {}", path.display()),
            source,
        }),
    }
}

/// `path` made absolute against the working directory, without resolving links.
fn absolute(path: &Path) -> Result<PathBuf, GitError> {
    std::path::absolute(path).map_err(|source| GitError::Io {
        context: format!("cannot make {} absolute", path.display()),
        source,
    })
}
