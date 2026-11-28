//! `mintswarm-runner tool pr-labels`: the label changes [`mintswarm_rules::pr_labels`] decides
//! for a PR, from git alone.
//!
//! It opens the working directory's repository as [`super::msw_vcs`] does, reads
//! `.mintswarm/config.yaml` at the base commit, never from the worktree, and takes the changed
//! paths from the diff between the merge base of base and head, and head. The base is a full
//! commit id; the head is `HEAD`, the default, or a full commit id, and nothing else, such as a
//! branch name or a short id, is resolved. Under the pinned `--no-renames` a rename is a deletion
//! and an addition, so both its paths count, a copy is an addition of its new path, and every
//! deleted path counts. The PR's labels and the repository's labels come from two
//! files holding GitHub's arrays of label objects, of which only `name` is read.
//!
//! On success it prints the rule's JSON on stdout and exits 0; any unusable input is one
//! `pr-labels: ` line on stderr, exit 2 and nothing on stdout. It makes no GitHub call.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use mintswarm_domain::records::{Band, Json};
use mintswarm_domain::vcs::{DiffOpts, RepoPath, Vcs};
use mintswarm_rules::exit::{Outcome, Unusable, Verdict};
use mintswarm_rules::json;
use mintswarm_rules::plan_decision::envelope::BANDS;
use mintswarm_rules::pr_labels;

use super::msw_vcs::{JobScope, Setup, checked_base};
use super::plan_args::{self, Spec};
use crate::exec::git::{GitError, GitStore};

/// The usage line, appended to the argument refusals.
pub const USAGE: &str = "usage: mintswarm-runner tool pr-labels --base COMMIT --pr-labels PATH \
                         --repo-labels PATH [--head COMMIT] [--band BAND]";

/// The config the rule reads, relative to the top of the repository.
pub const CONFIG: &str = ".mintswarm/config.yaml";

/// The arguments.
const SPEC: Spec = Spec {
    required: &["--base", "--pr-labels", "--repo-labels"],
    optional: &["--head", "--band"],
    usage: USAGE,
    issue: false,
};

/// Runs the tool on `argv`, the arguments after `tool pr-labels` exactly as given, in the
/// working directory `cwd`, writing the changes to `stdout` or the one error line to `stderr`.
/// Failures to write either are ignored.
pub fn run(
    argv: &[OsString],
    cwd: &Path,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Outcome {
    match decide(argv, cwd) {
        Ok(text) => {
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
            Ok(Verdict::Clean)
        }
        Err(unusable) => {
            let _ = writeln!(stderr, "pr-labels: {unusable}");
            Err(unusable)
        }
    }
}

/// The rule's JSON and a newline, or why there is none.
fn decide(argv: &[OsString], cwd: &Path) -> Result<String, Unusable> {
    let args = plan_args::parse(argv, &SPEC)?;
    let band = args.get("--band").map(band).transpose()?;
    let pr_labels = names(args.required("--pr-labels"), "--pr-labels")?;
    let repo_labels = names(args.required("--repo-labels"), "--repo-labels")?;

    let checkout = match GitStore::open_checkout(cwd) {
        Ok(checkout) => checkout,
        Err(GitError::NotInRepository(_)) => return Err(Unusable::new("not in a repository")),
        Err(error) => return Err(failure(&error)),
    };
    let store = &checkout.store;
    let base = checked_base(store, args.required("--base"))
        .map_err(|setup| setup_error(&setup, "--base"))?;
    let head = match args.get("--head") {
        None | Some("HEAD") => checkout
            .head
            .clone()
            .ok_or_else(|| Unusable::new("--head is not a commit"))?,
        Some(head) => checked_base(store, head).map_err(|setup| setup_error(&setup, "--head"))?,
    };
    let scope = JobScope::new(store, base, head).map_err(|setup| setup_error(&setup, ""))?;

    let config = store
        .read_blob(&scope.base, &RepoPath(CONFIG.as_bytes().to_vec()))
        .map_err(|error| failure(&error))?
        .ok_or_else(|| Unusable::new(format!("no {CONFIG} at the base")))?;
    let view = pr_labels::view(&config)?;

    let changes = store
        .diff(&scope.merge_base, &scope.head, DiffOpts::default(), &[])
        .map_err(|error| failure(&error))?;
    let mut paths = BTreeSet::new();
    for change in &changes.files {
        // `old_path` only guards against a diff that reports renames
        for path in std::iter::once(&change.path).chain(change.old_path.as_ref()) {
            let path = String::from_utf8(path.0.clone())
                .map_err(|_| Unusable::new("a changed path is not UTF-8"))?;
            paths.insert(path);
        }
    }
    let paths: Vec<String> = paths.into_iter().collect();

    let changes = pr_labels::changes(&view, &paths, band, &pr_labels, &repo_labels)?;
    let mut text = serde_json::to_string(&changes)
        .map_err(|_| Unusable::new("cannot serialise the changes"))?;
    text.push('\n');
    Ok(text)
}

/// The band `id` names, one of the assess stage's five identifiers.
fn band(id: &str) -> Result<Band, Unusable> {
    BANDS
        .iter()
        .find(|entry| entry.0 == id)
        .map(|entry| entry.1)
        .ok_or_else(|| {
            let ids: Vec<&str> = BANDS.iter().map(|entry| entry.0).collect();
            Unusable::new(format!("--band must be one of {}; {USAGE}", ids.join(", ")))
        })
}

/// The label names in the file at `path`, given as `flag`: an array of objects, each with a
/// string `name`; other keys are ignored.
fn names(path: &str, flag: &str) -> Result<Vec<String>, Unusable> {
    let invalid = || {
        Unusable::new(format!(
            "{flag} must hold an array of objects with a string name"
        ))
    };
    let bytes =
        std::fs::read(path).map_err(|_| Unusable::new(format!("cannot read {flag} {path}")))?;
    let text = String::from_utf8(bytes).map_err(|_| invalid())?;
    let Json::Array(labels) = json::read(&text).map_err(|_| invalid())? else {
        return Err(invalid());
    };
    labels
        .into_iter()
        .map(|label| match label {
            Json::Object(pairs) => match pairs.into_iter().find(|(key, _)| key == "name") {
                Some((_, Json::String(name))) => Ok(name),
                _ => Err(invalid()),
            },
            _ => Err(invalid()),
        })
        .collect()
}

/// The message of a set-up error; `flag` names the revision a revision error is about.
fn setup_error(setup: &Setup, flag: &str) -> Unusable {
    match setup {
        Setup::Base => Unusable::new(format!("{flag} is not a full commit id")),
        Setup::Head => Unusable::new(format!("{flag} is not a commit")),
        Setup::MergeBase => Unusable::new("base and head do not have exactly one merge base"),
        Setup::NotInRepository => Unusable::new("not in a repository"),
        Setup::MissingInput | Setup::InvalidInput | Setup::Failure => vcs_failure(),
    }
}

fn vcs_failure() -> Unusable {
    Unusable::new("the VCS failed")
}

/// The VCS failure, after logging what failed.
fn failure(error: &GitError) -> Unusable {
    tracing::warn!(error = %error, "the VCS failed");
    vcs_failure()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_band_identifier_is_accepted() {
        for (id, expected, ..) in BANDS {
            assert_eq!(band(id).expect("a band"), expected);
        }
        assert!(band("bogus").is_err());
        assert!(band("Safe").is_err());
    }

    #[test]
    fn revision_errors_name_the_flag() {
        assert!(
            setup_error(&Setup::Base, "--head")
                .message()
                .starts_with("--head ")
        );
    }
}
