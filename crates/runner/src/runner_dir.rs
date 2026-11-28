//! The runner directory, design §12: the directory a runner is started in, which holds its
//! `runner.yaml`, its store and the service file that starts it, and nothing else.
//!
//! [`claim`] makes a directory a runner directory (`mintswarm-runner join`), [`open`] opens one
//! to run in it (`mintswarm-runner run`), and [`add_service`] writes the systemd unit or launchd
//! plist that runs it (`mintswarm-runner add-service`). None of them reads the environment: the
//! caller passes the directory, which the binary takes from its working directory.

use std::ffi::OsStr;
use std::fs::{OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use mintswarm_config::{RunnerConfig, load_runner_config};
use sha2::{Digest, Sha256};

use crate::exec::store::{self, OutboxMode, Store, StoreError, SystemClock};

/// The runner config, at the top of the runner directory.
pub const RUNNER_YAML: &str = "runner.yaml";

/// What [`claim`] writes to a new `runner.yaml`: every key at its default.
const RUNNER_YAML_TEXT: &str = "\
# The mintswarm runner config of this runner directory.
# Every key is optional; a key that is absent takes its default.
{}
";

/// The start of a service file name and of a launchd `Label`.
const SERVICE_PREFIX: &str = "mintswarm-runner-";

/// How many hexadecimal digits of the directory's hash a service name carries.
const SERVICE_HASH_DIGITS: usize = 12;

/// The systemd unit template.
const UNIT_TEMPLATE: &str = include_str!("../../../packaging/systemd/mintswarm-runner.service");

/// The launchd plist template.
const PLIST_TEMPLATE: &str = include_str!("../../../packaging/launchd/mintswarm-runner.plist");

/// What went wrong with a runner directory.
#[derive(Debug, thiserror::Error)]
pub enum RunnerDirError {
    /// `join` in a directory that holds `runner.yaml` already.
    #[error("this directory is a runner directory already: it holds {RUNNER_YAML}")]
    AlreadyJoined,
    /// `join` in a directory that holds entries a runner does not own, by name, sorted.
    #[error("this directory holds files that are not a runner's: {}", names(.0))]
    ForeignFiles(Vec<PathBuf>),
    /// `run` or `add-service` in a directory without `runner.yaml`.
    #[error(
        "this directory is not a runner directory: it has no {RUNNER_YAML}; run `mintswarm-runner join` first"
    )]
    NotJoined,
    /// `runner.yaml` is not a valid runner config.
    #[error("{RUNNER_YAML}: {0}")]
    Config(mintswarm_config::Error),
    /// The store refused the directory, for example for its mode or its lock.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The service file is in the runner directory already.
    #[error("{} exists already; remove it to write it again", .0.display())]
    ServiceExists(PathBuf),
    /// A path a service file cannot hold: not UTF-8, with a control character, for the unit's
    /// `WorkingDirectory=` starting or ending with whitespace or ending with `\`, or for the
    /// unit's program holding `$`.
    #[error("{} cannot be written into a service file", .0.display())]
    UnrenderablePath(PathBuf),
    /// A file operation failed. The text is `context` alone; the underlying error is only the
    /// `source()`.
    #[error("{context}")]
    Io {
        /// What was being done.
        context: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// `names`, comma-separated, for [`RunnerDirError::ForeignFiles`].
fn names(names: &[PathBuf]) -> String {
    names
        .iter()
        .map(|name| name.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Map an IO error to [`RunnerDirError::Io`] with `what` and `path` as the context.
fn io(path: &Path, what: &str) -> impl FnOnce(std::io::Error) -> RunnerDirError {
    let context = format!("{what} {}", path.display());
    move |source| RunnerDirError::Io { context, source }
}

/// The kind of service file the host's init system loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    /// A systemd user unit, `.service`.
    Systemd,
    /// A launchd agent, `.plist`.
    Launchd,
}

impl ServiceKind {
    /// The kind for this host: launchd on macOS, systemd on every other host.
    pub fn host() -> ServiceKind {
        if cfg!(target_os = "macos") {
            ServiceKind::Launchd
        } else {
            ServiceKind::Systemd
        }
    }

    /// The file name extension, without the dot.
    fn extension(self) -> &'static str {
        match self {
            ServiceKind::Systemd => "service",
            ServiceKind::Launchd => "plist",
        }
    }
}

/// Whether a runner owns the top-level entry `name`: one of the store's entries,
/// `runner.yaml`, or a service file `mintswarm-runner-<12 hex digits>.service` or `.plist`.
pub fn is_owned(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    name == RUNNER_YAML || store::owned_entries().any(|owned| owned == name) || is_service(name)
}

/// Whether `name` is a service file name that [`add_service`] writes.
fn is_service(name: &str) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    let Some(hash) = stem.strip_prefix(SERVICE_PREFIX) else {
        return false;
    };
    [ServiceKind::Systemd, ServiceKind::Launchd]
        .iter()
        .any(|kind| kind.extension() == extension)
        && hash.len() == SERVICE_HASH_DIGITS
        && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Make `dir` a runner directory: `mintswarm-runner join`.
///
/// It fails with [`RunnerDirError::AlreadyJoined`] when `dir` holds `runner.yaml`, and with
/// [`RunnerDirError::ForeignFiles`] when it holds an entry a runner does not own, in both cases
/// without changing anything. Otherwise it sets `dir` to mode 0700, opens and closes the store
/// there, and writes a `runner.yaml` with every key at its default.
pub fn claim(dir: &Path) -> Result<(), RunnerDirError> {
    let entries = std::fs::read_dir(dir).map_err(io(dir, "cannot list"))?;
    let mut foreign = Vec::new();
    for entry in entries {
        let name = entry.map_err(io(dir, "cannot list"))?.file_name();
        if name == RUNNER_YAML {
            return Err(RunnerDirError::AlreadyJoined);
        }
        if !is_owned(&name) {
            foreign.push(PathBuf::from(name));
        }
    }
    if !foreign.is_empty() {
        foreign.sort();
        return Err(RunnerDirError::ForeignFiles(foreign));
    }
    std::fs::set_permissions(dir, Permissions::from_mode(0o700))
        .map_err(io(dir, "cannot set the mode of"))?;
    drop(Store::open(dir, SystemClock, OutboxMode::Suppressed)?);
    let path = dir.join(RUNNER_YAML);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(io(&path, "cannot create"))?;
    file.write_all(RUNNER_YAML_TEXT.as_bytes())
        .map_err(io(&path, "cannot write"))
}

/// The config in `dir`'s `runner.yaml`, or [`RunnerDirError::NotJoined`] when it has none.
fn read_config(dir: &Path) -> Result<RunnerConfig, RunnerDirError> {
    let path = dir.join(RUNNER_YAML);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(RunnerDirError::NotJoined);
        }
        Err(err) => return Err(io(&path, "cannot read")(err)),
    };
    load_runner_config(&text).map_err(RunnerDirError::Config)
}

/// Open the runner directory `dir` to run in it: `mintswarm-runner run`.
///
/// It fails with [`RunnerDirError::NotJoined`] before touching anything when `dir` has no
/// `runner.yaml`, and with [`RunnerDirError::Config`] when that is not a valid runner config.
/// It then opens the store, which refuses a directory of another mode than 0700 and one whose
/// lock another runner holds; the lock is held while the returned [`Store`] lives. The outbox is
/// suppressed, as there is no connection to the plane yet.
pub fn open(dir: &Path) -> Result<(RunnerConfig, Store), RunnerDirError> {
    let config = read_config(dir)?;
    let store = Store::open(dir, SystemClock, OutboxMode::Suppressed)?;
    Ok((config, store))
}

/// Check that `dir` is a runner directory that [`open`] would open, without taking its lock or
/// changing anything: a runner may be running in it.
pub fn check_service_dir(dir: &Path) -> Result<RunnerConfig, RunnerDirError> {
    let config = read_config(dir)?;
    store::check_root(dir)?;
    Ok(config)
}

/// The service name of the absolute runner directory `dir`: `mintswarm-runner-` and the first
/// 12 hexadecimal digits of the sha256 of its bytes. It names the unit file, the plist file and
/// the plist's `Label`.
pub fn service_name(dir: &Path) -> String {
    let digest = Sha256::digest(dir.as_os_str().as_bytes());
    let mut name = String::from(SERVICE_PREFIX);
    for byte in digest.iter().take(SERVICE_HASH_DIGITS / 2) {
        name.push_str(&format!("{byte:02x}"));
    }
    name
}

/// `template` with each `{{name}}` token replaced by the value of `name` in `values`, in one
/// pass: an inserted value is never scanned again. A token of another name stays as it is.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let value = after.find("}}").and_then(|end| {
            values
                .iter()
                .find(|(name, _)| *name == &after[..end])
                .map(|(_, value)| (end, *value))
        });
        match value {
            Some((end, value)) => {
                out.push_str(value);
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `path` as UTF-8 text without control characters, or [`RunnerDirError::UnrenderablePath`].
fn renderable(path: &Path) -> Result<&str, RunnerDirError> {
    path.to_str()
        .filter(|text| !text.chars().any(char::is_control))
        .ok_or_else(|| RunnerDirError::UnrenderablePath(path.to_path_buf()))
}

/// The systemd unit that runs `binary run` in the runner directory `dir`, both absolute.
///
/// `%` starts a specifier everywhere in a unit, so it is written `%%`. `ExecStart=` quotes the
/// binary in `"`, inside which `\` and `"` are escaped with `\`. A binary path holding `$` is
/// refused with [`RunnerDirError::UnrenderablePath`]: systemd expands variables in the
/// arguments but not in the program path, so neither `$` nor `$$` reaches the program path
/// intact. `WorkingDirectory=` takes the path unquoted: systemd strips leading and trailing
/// whitespace from a value and a trailing `\` continues the line, so a directory that starts or
/// ends with whitespace, or ends with `\`, is refused too, and so is a path that is not UTF-8 or
/// holds a control character.
pub fn render_unit(dir: &Path, binary: &Path) -> Result<String, RunnerDirError> {
    let directory = renderable(dir)?;
    if directory.starts_with(char::is_whitespace)
        || directory.ends_with(char::is_whitespace)
        || directory.ends_with('\\')
    {
        return Err(RunnerDirError::UnrenderablePath(dir.to_path_buf()));
    }
    let program = renderable(binary)?;
    if program.contains('$') {
        return Err(RunnerDirError::UnrenderablePath(binary.to_path_buf()));
    }
    let mut exec_start = String::from("\"");
    for c in program.chars() {
        match c {
            '\\' | '"' => {
                exec_start.push('\\');
                exec_start.push(c);
            }
            '%' => exec_start.push_str("%%"),
            c => exec_start.push(c),
        }
    }
    exec_start.push_str("\" run");
    let label = service_name(dir);
    Ok(fill(
        UNIT_TEMPLATE,
        &[
            ("label", &label),
            ("working_directory", &directory.replace('%', "%%")),
            ("exec_start", &exec_start),
        ],
    ))
}

/// `text` with the XML special characters written as entities.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The launchd plist that runs `binary run` in the runner directory `dir`, both absolute, with
/// the XML special characters as entities. A path that is not UTF-8 or holds a control
/// character is refused with [`RunnerDirError::UnrenderablePath`].
pub fn render_plist(dir: &Path, binary: &Path) -> Result<String, RunnerDirError> {
    let directory = xml_escape(renderable(dir)?);
    let program = xml_escape(renderable(binary)?);
    let label = service_name(dir);
    Ok(fill(
        PLIST_TEMPLATE,
        &[
            ("label", &label),
            ("working_directory", &directory),
            ("program", &program),
        ],
    ))
}

/// `text` in single quotes for a POSIX shell, each `'` written `'\''`.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The commands that load the service file `file` named `name`, as `add-service` prints them.
///
/// For launchd the plist is copied into `~/Library/LaunchAgents`, where a new login session
/// finds it, and the copy is bootstrapped. Whenever the file is written again the loaded agent
/// is booted out first, as a second bootstrap of a loaded label fails, and the copy and the
/// bootstrap are repeated. For systemd the unit is linked and enabled, with a note that a user unit stops
/// at logout unless lingering is on.
pub fn instructions(kind: ServiceKind, file: &Path, name: &str) -> String {
    let file = shell_quote(&file.to_string_lossy());
    let refresh =
        "If the binary moves, remove the file and run `mintswarm-runner add-service` again";
    match kind {
        ServiceKind::Systemd => format!(
            "To start the runner now and at every login:\n\
             \x20 systemctl --user link {file}\n\
             \x20 systemctl --user enable --now {name}\n\
             A user unit stops when you log out unless lingering is on: \
             loginctl enable-linger\n\
             {refresh}, then run systemctl --user daemon-reload and restart {name}.\n"
        ),
        ServiceKind::Launchd => {
            let agent = format!("~/Library/LaunchAgents/{name}.plist");
            let copy = format!("cp {file} {agent}");
            let bootstrap = format!("launchctl bootstrap gui/$(id -u) {agent}");
            format!(
                "To start the runner now and at every login:\n\
                 \x20 {copy}\n\
                 \x20 {bootstrap}\n\
                 launchd reads the copy. {refresh}; after `mintswarm-runner add-service` is run \
                 again, boot the loaded agent out, then repeat the cp and the bootstrap:\n\
                 \x20 launchctl bootout gui/$(id -u)/{name}\n\
                 \x20 {copy}\n\
                 \x20 {bootstrap}\n"
            )
        }
    }
}

/// Write the service file of the runner directory `dir` for `kind`, running `binary run`:
/// `mintswarm-runner add-service`. Both paths are made absolute first.
///
/// It refuses a directory that [`open`] would refuse, apart from the lock, and a service file
/// that is there already, which it leaves as it is. It writes only into `dir`, and returns the
/// file's path and the [`instructions`] that load it.
pub fn add_service(
    dir: &Path,
    binary: &Path,
    kind: ServiceKind,
) -> Result<(PathBuf, String), RunnerDirError> {
    let dir = std::path::absolute(dir).map_err(io(dir, "cannot resolve"))?;
    let binary = std::path::absolute(binary).map_err(io(binary, "cannot resolve"))?;
    check_service_dir(&dir)?;
    let text = match kind {
        ServiceKind::Systemd => render_unit(&dir, &binary)?,
        ServiceKind::Launchd => render_plist(&dir, &binary)?,
    };
    let name = service_name(&dir);
    let path = dir.join(format!("{name}.{}", kind.extension()));
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(RunnerDirError::ServiceExists(path));
        }
        Err(err) => return Err(io(&path, "cannot create")(err)),
    };
    file.write_all(text.as_bytes())
        .map_err(io(&path, "cannot write"))?;
    let instructions = instructions(kind, &path, &name);
    Ok((path, instructions))
}
