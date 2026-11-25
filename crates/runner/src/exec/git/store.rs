//! Creating and opening the trusted store: mode 0700, bare, neither shallow nor partial, no
//! alternates and no attributes of its own, and a git that meets [`GIT_REQUIREMENT`].

use std::fs::{DirBuilder, Permissions};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use mintswarm_domain::probe::{Check, ProbeResult, ProbeValue, VersionRequirement};

use super::{GitError, GitStore};

/// The git versions the store works with, checked against the `tool.git` probe result.
pub const GIT_REQUIREMENT: &str = ">= 2.45";

/// The probe whose result [`GitStore::open`] takes.
const GIT_PROBE: &str = "tool.git";

/// The mode of the store's directory.
const STORE_MODE: u32 = 0o700;

/// Words git reads as false in a boolean setting.
const FALSE_WORDS: &[&str] = &["false", "no", "off", "0"];

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
        store.id_len = match store.line(["rev-parse", "--show-object-format"])?.as_str() {
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
        store.attr_source = Some(store.line(["hash-object", "-t", "tree", "--stdin"])?);
        store.warnings = warnings;
        Ok(store)
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

    /// Allow `file://` remotes: the test-only remote.
    #[cfg(test)]
    pub(super) fn allow_file_remote(&mut self) {
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
fn non_empty(path: &Path) -> Result<bool, GitError> {
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
