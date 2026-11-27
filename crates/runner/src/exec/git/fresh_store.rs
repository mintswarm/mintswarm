//! A fresh object store for the plan floor: the base of a working directory's repository,
//! fetched by id into a new bare store in the system temporary directory, as
//! `plan-approval/floor.py`'s `job_object_store` makes one.
//!
//! The working directory's repository is not trusted: a branch under review could hold replace
//! refs, grafts, a patched commit-graph, a forged loose object or a forged `shallow` file. So
//! the reference is resolved there without peeling, the id is fetched over `file://` with
//! protocol v2 into a store of its own, which re-hashes every object, the fetched id's closure
//! is walked in that store, and only then is the id peeled to its commit. The store is removed
//! when the [`FreshStore`] is dropped, on every path, a panic included.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Stdio;

use mintswarm_domain::ids::RevId;
use mintswarm_domain::probe::ProbeResult;
use mintswarm_domain::records::BaseRef;
use mintswarm_rules::exit::Unusable;
use mintswarm_rules::py::repr_str;
use tempfile::TempDir;

use super::env::status_text;
use super::read::id_text;
use super::{GitError, GitStore};

/// The prefix of the temporary directory that holds the store, `floor.py`'s `STORE_PREFIX`.
pub const STORE_PREFIX: &str = "msw-floor-objects-";

/// The fetch's options, `floor.py`'s `FETCH_OPTIONS`.
const FETCH_OPTIONS: &[&str] = &[
    "fetch",
    "--quiet",
    "--no-tags",
    "--no-write-commit-graph",
    "--no-auto-gc",
    "--no-auto-maintenance",
];

/// What serves the fetch, run by git through the shell, `floor.py`'s `UPLOAD_PACK`: the local
/// transport drops the repository-local variables, so these are set again for it, as shell
/// assignments.
const UPLOAD_PACK: &str = "--upload-pack=GIT_GRAFT_FILE=/dev/null GIT_NO_REPLACE_OBJECTS=1 \
                           GIT_NO_LAZY_FETCH=1 git -c core.commitGraph=false \
                           -c advice.graftFileDeprecated=false upload-pack";

/// Why a fresh object store cannot be had. The text is ours alone; what git wrote is never in
/// it, and a git or file error is only the `source()`.
#[derive(Debug, thiserror::Error)]
pub enum FreshStoreError {
    /// The job repository cannot be opened or read.
    #[error("cannot read the job repository")]
    Repository(#[source] GitError),
    /// The job repository is a partial clone, so a missing object could be fetched lazily.
    #[error("the repository is a partial clone ({0} is set): its objects are not read")]
    Partial(String),
    /// The reference names no object in the job repository, or the store cannot peel it to a
    /// commit.
    #[error("base ref {} does not resolve to a commit", repr_str(.0))]
    Unresolved(String),
    /// The job repository's common git directory is not an absolute path.
    #[error("the job repository's git directory {} is not an absolute path", repr_str(.0))]
    CommonDir(String),
    /// The fresh store cannot be made or read.
    #[error("cannot use a fresh object store")]
    Store(#[source] GitError),
    /// A step of filling the store failed.
    #[error(
        "the fetch from the job repository into a fresh object store failed (git {command}, {status})"
    )]
    Fetch {
        /// The git subcommand.
        command: &'static str,
        /// Its exit status, in our words: `exit code N` or `no exit code`.
        status: String,
    },
    /// The fetch gave a shallow store.
    #[error("the fetch from the job repository gave a shallow history")]
    Shallow,
}

impl FreshStoreError {
    /// The tool's unusable input for this error: exit class 2.
    pub fn unusable(&self) -> Unusable {
        Unusable::new(self.to_string())
    }
}

/// The base of a working directory's repository in a fresh bare store of its own, removed when
/// this is dropped.
#[derive(Debug)]
pub struct FreshStore {
    // Declared before `dir`, so the store is dropped before its directory is removed.
    store: GitStore,
    base: BaseRef,
    dir: TempDir,
}

impl FreshStore {
    /// Fetch the object `reference` names in the repository whose working tree holds `dir` into
    /// a fresh bare store, made in a `msw-floor-objects-` directory of the system temporary
    /// directory, and peel it to its commit there. `git` is the `tool.git` probe result the
    /// store is opened with.
    ///
    /// In order: a partial clone is refused; `reference` is resolved in the repository without
    /// peeling; its common git directory must be absolute; the id is fetched over a
    /// percent-encoded `file://` URL of that directory with protocol v2; a shallow store is
    /// refused; the id's closure is walked; the id is peeled to its commit in the store.
    pub fn from_checkout(
        dir: &Path,
        reference: &str,
        git: &ProbeResult,
    ) -> Result<FreshStore, FreshStoreError> {
        let checkout = GitStore::open_checkout(dir).map_err(FreshStoreError::Repository)?;
        let job = checkout.store;
        if let Some(key) = job.promisor_key().map_err(FreshStoreError::Repository)? {
            return Err(FreshStoreError::Partial(key));
        }
        let id = resolve_object(&job, reference)?;
        let common = job
            .git(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .map_err(FreshStoreError::Repository)?;
        let common = common.strip_suffix(b"\n").unwrap_or(&common);
        if !common.starts_with(b"/") {
            return Err(FreshStoreError::CommonDir(
                String::from_utf8_lossy(common).into_owned(),
            ));
        }
        let url = format!("file://{}", quote(common));

        let temporary = tempfile::Builder::new()
            .prefix(STORE_PREFIX)
            .tempdir_in(std::env::temp_dir())
            .map_err(|source| {
                FreshStoreError::Store(GitError::Io {
                    context: "cannot make a temporary directory".to_owned(),
                    source,
                })
            })?;
        let mut store = GitStore::create(&temporary.path().join("objects.git"), git)
            .map_err(FreshStoreError::Store)?;
        // Only this private store fetches over file://, from the job repository alone.
        store.allow_file_remote();

        let mut args: Vec<&str> = FETCH_OPTIONS.to_vec();
        args.extend([UPLOAD_PACK, url.as_str(), id.as_str()]);
        let fetch = store.command_with(&["protocol.version=2"], &args);
        step(&store, fetch, "fetch")?;
        let shallow = store
            .git(["rev-parse", "--is-shallow-repository"])
            .map_err(FreshStoreError::Store)?;
        if shallow != b"false\n" {
            return Err(FreshStoreError::Shallow);
        }
        // A `shallow` file in the job repository cuts what upload-pack sends without marking the
        // store shallow, so the closure is walked: a missing object fails the walk.
        let mut walk = store.command(["rev-list", "--objects", id.as_str(), "--"]);
        walk.stdout(Stdio::null());
        step(&store, walk, "rev-list")?;

        let commit = peel(&store, &id).ok_or_else(|| unresolved(reference))?;
        Ok(FreshStore {
            store,
            base: BaseRef {
                reference: reference.to_owned(),
                commit,
            },
            dir: temporary,
        })
    }

    /// The fresh store.
    pub fn store(&self) -> &GitStore {
        &self.store
    }

    /// The reference as given, and the commit it peeled to in the fresh store.
    pub fn base(&self) -> &BaseRef {
        &self.base
    }

    /// The temporary directory that holds the store.
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
}

/// The refusal of `reference`.
fn unresolved(reference: &str) -> FreshStoreError {
    FreshStoreError::Unresolved(reference.to_owned())
}

/// The full id of the object `reference` names in the job repository, not peeled.
fn resolve_object(job: &GitStore, reference: &str) -> Result<String, FreshStoreError> {
    let spec = format!("{reference}^{{object}}");
    let output = job
        .output(
            job.command([
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                spec.as_str(),
            ]),
            "rev-parse",
        )
        .map_err(FreshStoreError::Repository)?;
    if !output.status.success() {
        return Err(unresolved(reference));
    }
    let id = String::from_utf8_lossy(output.stdout.trim_ascii()).into_owned();
    if job.is_full_id(&id) {
        Ok(id)
    } else {
        Err(unresolved(reference))
    }
}

/// Run one step of filling the store; any exit status but 0 is the fetch's failure.
fn step(
    store: &GitStore,
    command: std::process::Command,
    name: &'static str,
) -> Result<(), FreshStoreError> {
    let output = store
        .output(command, name)
        .map_err(FreshStoreError::Store)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(FreshStoreError::Fetch {
            command: name,
            status: status_text(&output),
        })
    }
}

/// The commit `id` peels to in the store, or `None` when it peels to none.
fn peel(store: &GitStore, id: &str) -> Option<RevId> {
    let spec = format!("{id}^{{commit}}");
    let output = store
        .output(
            store.command([
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                spec.as_str(),
            ]),
            "rev-parse",
        )
        .ok()?;
    if !output.status.success() {
        return None;
    }
    id_text(store, output.stdout.trim_ascii(), "rev-parse")
        .ok()
        .map(RevId)
}

/// `path` percent-encoded as `urllib.parse.quote` encodes it: ASCII letters and digits, `_`,
/// `.`, `-`, `~` and `/` kept, every other byte as `%XX`.
pub(super) fn quote(path: &[u8]) -> String {
    let mut out = String::with_capacity(path.len());
    for &byte in path {
        if byte.is_ascii_alphanumeric() || b"_.-~/".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}
