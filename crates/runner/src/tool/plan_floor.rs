//! `mintswarm-runner tool plan-floor`, the Rust port of `.mintswarm/dogfood/plan-approval/floor.py`.
//!
//! It deletes a stale `.mintswarm/job/outputs/floor.json` first, checks its arguments with
//! [`super::plan_args`], reads the base from a [`FreshStore`] of the working directory's
//! repository, builds the bundle with [`plan_floor_bundle`], applies
//! [`mintswarm_rules::plan_floor::floor`], writes `floor.json` atomically, and prints one line
//! per rule and then `floor: hit` or `floor: clean`. The arguments, files, stdout and exit codes
//! follow `floor.py`; stderr is one `plan-floor: ` line in Rust's wording, and it runs no `yq`
//! and no Python.

use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::path::Path;

use mintswarm_domain::probe::{ProbeResult, ProbeValue};
use mintswarm_domain::records::Verdict as RuleVerdict;
use mintswarm_rules::exit::{Outcome, Unusable};
use mintswarm_rules::plan_floor;

use super::plan_args::{self, Spec};
use super::save_output::write_both;
use crate::exec::git::{Documents, FreshStore, plan_floor_bundle};

/// The usage line, appended to the argument refusals.
pub const USAGE: &str = "usage: mintswarm-runner tool plan-floor --plan PATH --components NAMES \
                         --base REF [--issue-document PATH] [--analysis PATH] [--screen PATH]";

/// The verdict, relative to the working directory.
pub const VERDICT: &str = ".mintswarm/job/outputs/floor.json";

/// The arguments, `floor.py`'s `REQUIRED` and `OPTIONAL`.
const SPEC: Spec = Spec {
    required: &["--plan", "--components", "--base"],
    optional: &["--issue-document", "--analysis", "--screen"],
    usage: USAGE,
    issue: false,
};

/// The `tool.git` probe result the fresh store is opened with, until the runner probes git.
pub(super) fn git_probe() -> ProbeResult {
    ProbeResult {
        namespace: "tool.git".parse().expect("tool.git is a probe namespace"),
        value: ProbeValue::Unknown,
        observed_at: String::new(),
    }
}

/// Delete `path` under `cwd` when it exists; `false` when it exists and cannot be deleted, a
/// directory included.
pub(super) fn remove_stale(cwd: &Path, path: &str) -> bool {
    match std::fs::remove_file(cwd.join(path)) {
        Ok(()) => true,
        Err(error) => error.kind() == ErrorKind::NotFound,
    }
}

/// Runs the tool on `argv`, the arguments after `tool plan-floor` exactly as given, in the
/// working directory `cwd`, writing what it reports to `stdout` and `stderr`. Failures to write
/// either are ignored.
pub fn run(
    argv: &[OsString],
    cwd: &Path,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Outcome {
    match floor(argv, cwd) {
        Ok((lines, outcome)) => {
            let _ = stdout.write_all(lines.as_bytes());
            outcome
        }
        Err(unusable) => {
            let _ = writeln!(stderr, "plan-floor: {unusable}");
            Err(unusable)
        }
    }
}

/// The floor's stdout and outcome, or why there is no verdict.
fn floor(argv: &[OsString], cwd: &Path) -> Result<(String, Outcome), Unusable> {
    if !remove_stale(cwd, VERDICT) {
        return Err(Unusable::new(format!(
            "cannot delete the stale verdict {VERDICT}"
        )));
    }
    let args = plan_args::parse(argv, &SPEC)?;
    let components = args.components();
    let documents = Documents {
        plan: args.required("--plan").to_owned(),
        issue_document: args.get("--issue-document").map(str::to_owned),
        analysis: args.get("--analysis").map(str::to_owned),
        screen: args.get("--screen").map(str::to_owned),
    };
    let fresh = FreshStore::from_checkout(cwd, args.required("--base"), &git_probe())
        .map_err(|error| error.unusable())?;
    let bundle = plan_floor_bundle(
        fresh.store(),
        fresh.base().clone(),
        &components,
        cwd,
        &documents,
    )
    .map_err(|error| error.unusable())?;
    drop(fresh);
    let floor = plan_floor::floor(&bundle)?;
    write_both(&[(cwd.join(VERDICT), &floor.text())])
        .map_err(|_| Unusable::new(format!("cannot write the verdict {VERDICT}")))?;

    let record = floor.record();
    let mut lines = String::new();
    for rule in &record.rules {
        let result = match rule.result {
            RuleVerdict::Hit => "hit",
            RuleVerdict::Clean => "clean",
        };
        lines.push_str(&format!("{result:<7} {}", rule.rule));
        if let Some(first) = rule.reasons.first() {
            lines.push_str(&format!(": {first}"));
        }
        lines.push('\n');
    }
    let verdict = match record.verdict {
        RuleVerdict::Hit => "hit",
        RuleVerdict::Clean => "clean",
    };
    lines.push_str(&format!("floor: {verdict}\n"));
    Ok((lines, floor.outcome()))
}
