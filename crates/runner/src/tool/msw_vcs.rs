//! `msw-vcs`, the read tool through which stages read the job's history, and
//! `mintswarm-runner tool msw-vcs`, the same CLI over the working directory's repository.
//!
//! One request pipeline serves both the tool mode and the per-attempt server of
//! [`crate::exec::tools`]: [`args::parse`], then the job's set-up ([`JobScope`]), then
//! [`execute`], so the order of the checks and every message have one source. The contract is
//! `.mintswarm/dogfood/msw-vcs.md`, and the tool's stdout, stderr and exit code equal the dogfood
//! shim's on every case of its self-test.
//!
//! The output is written from the domain's records through [`format`], so it does not depend on
//! the VCS. A VCS failure is logged through `tracing` and reported as `vcs failure` alone.

pub mod args;
pub mod format;

use std::ffi::OsString;
use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::path::Path;

use mintswarm_domain::ids::RevId;
use mintswarm_domain::records::Json;
use mintswarm_domain::vcs::{DiffOpts, RevRange, TreeEntry, Vcs};
use mintswarm_rules::json;

use crate::exec::git::{GitError, GitStore};
use args::{Fail, Form, LogRange, Request, Rev};

/// The shim's job input, relative to the top of the repository.
pub const VCS_JSON: &str = ".mintswarm/job/inputs/vcs.json";

/// A job set-up error: exit code 4, or 5 when the VCS failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setup {
    /// The working directory is not in a repository.
    NotInRepository,
    /// The shim's job input is missing.
    MissingInput,
    /// It is not UTF-8 JSON, not an object, or has no string `base`.
    InvalidInput,
    /// `base` is not a full id of a commit.
    Base,
    /// The worktree has no `HEAD` commit.
    Head,
    /// Base and head do not have exactly one merge base.
    MergeBase,
    /// The VCS failed.
    Failure,
}

impl Setup {
    /// The failure the call ends with.
    pub fn fail(&self) -> Fail {
        match self {
            Setup::NotInRepository => Fail::setup("not in a repository"),
            Setup::MissingInput => Fail::setup(&format!("missing job input {VCS_JSON}")),
            Setup::InvalidInput => Fail::setup(&format!("invalid job input {VCS_JSON}")),
            Setup::Base => Fail::setup(&format!("base in {VCS_JSON} is not a full commit id")),
            Setup::Head => Fail::setup("head is not a commit"),
            Setup::MergeBase => Fail::setup("base and head do not have exactly one merge base"),
            Setup::Failure => Fail::vcs_failure(),
        }
    }
}

/// The job's three named revisions, the scope every revision is resolved in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobScope {
    /// The job's base commit.
    pub base: RevId,
    /// The job's head commit.
    pub head: RevId,
    /// The single merge base of base and head.
    pub merge_base: RevId,
}

impl JobScope {
    /// The scope of `base` and `head`, which must have exactly one merge base.
    pub fn new<V>(vcs: &V, base: RevId, head: RevId) -> Result<JobScope, Setup>
    where
        V: Vcs<Error = GitError>,
    {
        let mut bases = vcs.merge_bases(&base, &head).map_err(|error| {
            log_failure(&error);
            Setup::Failure
        })?;
        match (bases.pop(), bases.is_empty()) {
            (Some(merge_base), true) => Ok(JobScope {
                base,
                head,
                merge_base,
            }),
            _ => Err(Setup::MergeBase),
        }
    }

    /// The full id `rev` names in the scope; `arg` is quoted in the message of an unknown one.
    fn resolve<V>(&self, vcs: &V, rev: &Rev, arg: &[u8]) -> Result<RevId, Fail>
    where
        V: Vcs<Error = GitError>,
    {
        let id = match rev {
            Rev::Base => return Ok(self.base.clone()),
            Rev::Head => return Ok(self.head.clone()),
            Rev::MergeBase => return Ok(self.merge_base.clone()),
            Rev::Id(id) => id,
        };
        let id = match vcs.resolve(id) {
            Ok(id) => id,
            Err(GitError::NotFound(_) | GitError::RevisionSyntax(_)) => {
                return Err(Fail::unknown(arg));
            }
            Err(error) => return Err(failure(&error)),
        };
        let reachable = vcs.is_ancestor(&id, &self.base).map_err(|e| failure(&e))?
            || vcs.is_ancestor(&id, &self.head).map_err(|e| failure(&e))?;
        if reachable {
            Ok(id)
        } else {
            Err(Fail::unknown(arg))
        }
    }
}

fn log_failure(error: &GitError) {
    tracing::warn!(error = %error, "msw-vcs: the VCS failed");
}

/// `vcs failure`, after logging what failed.
fn failure(error: &GitError) -> Fail {
    log_failure(error);
    Fail::vcs_failure()
}

/// Run a parsed request in the job's scope; the bytes written to stdout on success.
pub fn execute<V>(request: &Request, scope: &JobScope, vcs: &V) -> Result<Vec<u8>, Fail>
where
    V: Vcs<Error = GitError>,
{
    match request {
        Request::Log { range, paths } => {
            let range = match range {
                None => RevRange {
                    from: Some(scope.merge_base.clone()),
                    to: scope.head.clone(),
                },
                Some(LogRange { from, to, arg }) => RevRange {
                    from: from
                        .as_ref()
                        .map(|from| scope.resolve(vcs, from, arg))
                        .transpose()?,
                    to: scope.resolve(vcs, to, arg)?,
                },
            };
            let commits = vcs.log(&range, paths).map_err(|e| failure(&e))?;
            Ok(format::log(&commits))
        }
        Request::Diff { form, revs, paths } => {
            let (old, new) = match revs {
                None => (scope.merge_base.clone(), scope.head.clone()),
                Some([(old, old_arg), (new, new_arg)]) => (
                    scope.resolve(vcs, old, old_arg)?,
                    scope.resolve(vcs, new, new_arg)?,
                ),
            };
            let opts = DiffOpts {
                numstat: *form == Form::Numstat,
                added: false,
                raw: *form == Form::Raw,
                patch: *form == Form::Patch,
            };
            let changes = vcs.diff(&old, &new, opts, paths).map_err(|e| failure(&e))?;
            Ok(match form {
                Form::Patch => changes.patch.unwrap_or_default(),
                Form::Numstat => format::numstat(&changes.files),
                Form::NameOnly => format::name_only(&changes.files),
                Form::Raw => format::raw(&changes.files),
            })
        }
        Request::Show { rev, path, arg } => {
            let commit = scope.resolve(vcs, rev, arg)?;
            match path {
                None => {
                    let info = vcs.read_commit(&commit).map_err(|e| failure(&e))?;
                    Ok(format::commit(&info))
                }
                Some(path) => show_path(vcs, &commit, path, arg),
            }
        }
    }
}

/// `show REV:PATH`: a file's bytes, a gitlink alone, or the files under a directory; the whole
/// tree for an empty path.
fn show_path<V>(vcs: &V, commit: &RevId, path: &[u8], arg: &[u8]) -> Result<Vec<u8>, Fail>
where
    V: Vcs<Error = GitError>,
{
    let entries = vcs.list_tree(commit).map_err(|e| failure(&e))?;
    if path.is_empty() {
        return Ok(format::tree(&entries));
    }
    if let Some(entry) = entries.iter().find(|entry| entry.path.0 == path) {
        if entry.mode == format::GITLINK {
            return Ok(format::gitlink(entry));
        }
        return match vcs.read_blob(commit, &entry.path) {
            Ok(Some(bytes)) => Ok(bytes),
            Ok(None) => Err(Fail::unknown(arg)),
            Err(error) => Err(failure(&error)),
        };
    }
    let mut prefix = path.to_vec();
    prefix.push(b'/');
    let under: Vec<&TreeEntry> = entries
        .iter()
        .filter(|entry| entry.path.0.starts_with(&prefix))
        .collect();
    if under.is_empty() {
        return Err(Fail::unknown(arg));
    }
    Ok(format::tree(under))
}

/// `mintswarm-runner tool msw-vcs`: the CLI over the repository whose working tree holds `cwd`,
/// with the job's base from its `.mintswarm/job/inputs/vcs.json` and its head at `HEAD`, set up
/// in the shim's order. Writes the output or the one error line and returns the exit code.
pub fn run(argv: &[OsString], cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let argv: Vec<Vec<u8>> = argv.iter().cloned().map(OsString::into_vec).collect();
    let result = args::parse(&argv).and_then(|request| {
        let checkout = match GitStore::open_checkout(cwd) {
            Ok(checkout) => checkout,
            Err(GitError::NotInRepository(_)) => return Err(Setup::NotInRepository.fail()),
            Err(error) => return Err(failure(&error)),
        };
        let store = &checkout.store;
        let base = read_base(&checkout.top.join(VCS_JSON)).map_err(|setup| setup.fail())?;
        let base = checked_base(store, &base).map_err(|setup| setup.fail())?;
        let head = checkout.head.clone().ok_or_else(|| Setup::Head.fail())?;
        let scope = JobScope::new(store, base, head).map_err(|setup| setup.fail())?;
        execute(&request, &scope, store)
    });
    let written = match result {
        Ok(bytes) => out.write_all(&bytes).and_then(|()| out.flush()).map(|()| 0),
        Err(fail) => err
            .write_all(&fail.line())
            .and_then(|()| err.flush())
            .map(|()| fail.exit),
    };
    // stdout or stderr closed early: the shim dies of the broken pipe, so exit 1 here
    written.unwrap_or(1)
}

/// The `base` string of the job input at `path`.
fn read_base(path: &Path) -> Result<String, Setup> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(Setup::MissingInput),
        Err(_) => return Err(Setup::InvalidInput),
    };
    let text = String::from_utf8(bytes).map_err(|_| Setup::InvalidInput)?;
    match json::read(&text).map_err(|_| Setup::InvalidInput)? {
        Json::Object(pairs) => match pairs.into_iter().find(|(key, _)| key == "base") {
            Some((_, Json::String(base))) => Ok(base),
            _ => Err(Setup::InvalidInput),
        },
        _ => Err(Setup::InvalidInput),
    }
}

/// `base` as the id of a commit, when it is a full lower-case id naming one.
fn checked_base(store: &GitStore, base: &str) -> Result<RevId, Setup> {
    let full = matches!(base.len(), 40 | 64)
        && base
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    if !full {
        return Err(Setup::Base);
    }
    match store.resolve(base) {
        Ok(id) if id.0 == base => Ok(id),
        Ok(_) | Err(GitError::NotFound(_) | GitError::RevisionSyntax(_)) => Err(Setup::Base),
        Err(error) => {
            log_failure(&error);
            Err(Setup::Failure)
        }
    }
}
