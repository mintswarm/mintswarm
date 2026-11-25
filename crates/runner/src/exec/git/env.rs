//! The neutral git environment of design §8 and the one command builder that applies it.
//!
//! It is the union of the dogfood scripts' environments (`landing/floor.py`,
//! `landing/keep-round.py`, `msw-vcs` and `github/common.sh`): the parent environment is
//! inherited, the variables below are removed, and fixed values are set.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::process::{Command, Output, Stdio};

use super::{GitError, GitStore};

/// Variables removed from every git command's environment. `GIT_DIR` and `GIT_ATTR_SOURCE` are
/// then set by the adapter.
pub(super) const DROPPED_ENV: &[&str] = &[
    // Configuration from the environment.
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_SYSTEM",
    // Diff drivers and options.
    "GIT_EXTERNAL_DIFF",
    "GIT_DIFF_OPTS",
    // Where the repository and its objects are.
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_SHALLOW_FILE",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_ATTR_SOURCE",
    "GIT_DEFAULT_HASH",
    "GIT_DEFAULT_REF_FORMAT",
    "GIT_TEMPLATE_DIR",
    // Tracing, which could write a request or a credential to a file.
    "GIT_CURL_VERBOSE",
    // Prompts and transports.
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_PROTOCOL_FROM_USER",
    "GIT_SMART_HTTP",
    // Which programs git runs.
    "GIT_EXEC_PATH",
    // Pathspec magic.
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
    // The credential helper's input, set only on a fetch that has a credential.
    "MSW_TOKEN",
];

/// Prefixes of further variables removed from every git command's environment: configuration,
/// tracing, and the TLS and HTTP settings of the connection that carries the credential.
pub(super) const DROPPED_ENV_PREFIXES: &[&str] = &[
    "GIT_CONFIG_KEY_",
    "GIT_CONFIG_VALUE_",
    "GIT_TRACE",
    "GIT_SSL_",
    "GIT_HTTP_",
];

/// Variables every git command gets, whatever the parent environment holds.
pub(super) const FIXED_ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_GRAFT_FILE", "/dev/null"),
    ("GIT_ATTR_NOSYSTEM", "1"),
    ("GIT_NO_REPLACE_OBJECTS", "1"),
    ("GIT_NO_LAZY_FETCH", "1"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_LITERAL_PATHSPECS", "1"),
    ("LC_ALL", "C"),
];

/// Settings every git command gets on its command line, over the store's own config.
pub(super) const CONFIG: &[&str] = &[
    "core.quotePath=false",
    "core.fsmonitor=false",
    "core.untrackedCache=false",
    "core.excludesFile=/dev/null",
    "core.attributesFile=/dev/null",
    "core.ignoreCase=false",
    "core.commitGraph=false",
    "core.bigFileThreshold=8g",
    "core.hooksPath=/dev/null",
    "core.askPass=",
    "diff.suppressBlankEmpty=false",
    "advice.graftFileDeprecated=false",
    "gc.auto=0",
    "maintenance.auto=false",
    "protocol.allow=never",
    "protocol.https.allow=always",
    "credential.helper=",
];

impl GitStore {
    /// A git command on the store, with the neutral environment and settings.
    pub(super) fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command_with(&[], args)
    }

    /// [`Self::command`] with further `-c` settings after the fixed ones.
    pub(super) fn command_with<I, S>(&self, config: &[&str], args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(&self.program);
        for (name, _) in std::env::vars_os() {
            let bytes = name.as_bytes();
            if DROPPED_ENV_PREFIXES
                .iter()
                .any(|prefix| bytes.starts_with(prefix.as_bytes()))
            {
                command.env_remove(&name);
            }
        }
        for name in DROPPED_ENV {
            command.env_remove(name);
        }
        command.envs(FIXED_ENV.iter().copied());
        command.env("GIT_DIR", &self.path);
        if let Some(tree) = &self.attr_source {
            command.env("GIT_ATTR_SOURCE", tree);
        }
        let (protocols, file) = if self.allow_file {
            ("https:file", "protocol.file.allow=always")
        } else {
            ("https", "protocol.file.allow=never")
        };
        command.env("GIT_ALLOW_PROTOCOL", protocols);
        command.args(["--literal-pathspecs", "--no-optional-locks"]);
        for item in CONFIG.iter().chain([&file]).chain(config) {
            command.arg("-c").arg(item);
        }
        command.args(args);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Run a command and return its output, whatever its exit status.
    pub(super) fn output(&self, mut command: Command, name: &str) -> Result<Output, GitError> {
        command.output().map_err(|source| GitError::Io {
            context: format!("cannot run git {name}"),
            source,
        })
    }

    /// Run a git subcommand and return its standard output; any exit status but 0 is an error.
    pub(super) fn git<I, S>(&self, args: I) -> Result<Vec<u8>, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<S> = args.into_iter().collect();
        let name = args
            .first()
            .map(|arg| arg.as_ref().to_string_lossy().into_owned())
            .unwrap_or_default();
        let output = self.output(self.command(&args), &name)?;
        success(&name, output)
    }
}

/// The standard output of a command that must succeed.
pub(super) fn success(name: &str, output: Output) -> Result<Vec<u8>, GitError> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(failure(name, &output))
    }
}

/// The error for a git command that failed.
pub(super) fn failure(name: &str, output: &Output) -> GitError {
    const LIMIT: usize = 500;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    let stderr = match stderr.char_indices().nth(LIMIT) {
        Some((end, _)) => format!("{}…", &stderr[..end]),
        None => stderr.to_owned(),
    };
    GitError::Git {
        command: name.to_owned(),
        status: output.status.code().map_or_else(
            || "no exit code".to_owned(),
            |code| format!("exit code {code}"),
        ),
        stderr,
    }
}
