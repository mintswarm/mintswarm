//! `mintswarm-runner tool plan-decision`, the Rust port of
//! `.mintswarm/dogfood/plan-approval/decision.py`.
//!
//! It deletes a stale `.mintswarm/job/outputs/decision.json` first, checks its arguments with
//! [`super::plan_args`], builds the floor's bundle from a [`FreshStore`] as
//! [`super::plan_floor`] does, reads the floor verdict and the assessment, applies
//! [`mintswarm_rules::plan_decision::decide`], writes `decision.json` atomically, prints the
//! `decision:` line to stderr and the comment, when there is one, to stdout. The arguments,
//! files, stdout and exit codes follow `decision.py`; an unusable run's stderr is one
//! `plan-decision: ` line in Rust's wording, and it runs no `yq` and no Python.

use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::path::Path;

use mintswarm_domain::records::{Band, Decision, LayerStatus, Mode, Verdict as RuleVerdict};
use mintswarm_rules::exit::{Outcome, Unusable};
use mintswarm_rules::plan_decision::{self, Input, Inputs};

use super::plan_args::{self, Spec};
use super::plan_floor::{self, git_probe, remove_stale};
use super::save_output::write_both;
use crate::exec::git::{Documents, FreshStore, plan_floor_bundle};

/// The usage line, appended to the argument refusals.
pub const USAGE: &str = "usage: mintswarm-runner tool plan-decision --issue N --components NAMES \
                         --plan PATH --issue-document PATH --analysis PATH --assessment PATH \
                         --assessment-artifact REF --base REF --harness NAME --model NAME \
                         [--screen PATH]";

/// The record, relative to the working directory.
pub const RECORD: &str = ".mintswarm/job/outputs/decision.json";

/// The arguments, `decision.py`'s `REQUIRED` and `OPTIONAL`.
const SPEC: Spec = Spec {
    required: &[
        "--issue",
        "--components",
        "--plan",
        "--issue-document",
        "--analysis",
        "--assessment",
        "--assessment-artifact",
        "--base",
        "--harness",
        "--model",
    ],
    optional: &["--screen"],
    usage: USAGE,
    issue: true,
};

/// Runs the tool on `argv`, the arguments after `tool plan-decision` exactly as given, in the
/// working directory `cwd`, writing what it reports to `stdout` and `stderr`. Failures to write
/// either are ignored.
pub fn run(
    argv: &[OsString],
    cwd: &Path,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Outcome {
    match decision(argv, cwd) {
        Ok((line, comment, outcome)) => {
            let _ = writeln!(stderr, "{line}");
            if let Some(comment) = comment {
                let _ = stdout.write_all(comment.as_bytes());
                let _ = stdout.flush();
            }
            outcome
        }
        Err(unusable) => {
            let _ = writeln!(stderr, "plan-decision: {unusable}");
            Err(unusable)
        }
    }
}

/// The `decision:` line, the comment and the outcome, or why there is no record.
fn decision(argv: &[OsString], cwd: &Path) -> Result<(String, Option<String>, Outcome), Unusable> {
    if !remove_stale(cwd, RECORD) {
        return Err(Unusable::new(format!(
            "cannot delete the stale record {RECORD}"
        )));
    }
    let args = plan_args::parse(argv, &SPEC)?;
    let issue_text = args.required("--issue");
    let issue: u64 = issue_text.parse().map_err(|_| {
        Unusable::new(format!(
            "--issue must be a positive decimal integer: {}",
            mintswarm_rules::py::repr_str(issue_text)
        ))
    })?;
    let components = args.components();
    let documents = Documents {
        plan: args.required("--plan").to_owned(),
        issue_document: Some(args.required("--issue-document").to_owned()),
        analysis: Some(args.required("--analysis").to_owned()),
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

    let verdict = read_input(&cwd.join(plan_floor::VERDICT));
    let assessment_path = args.required("--assessment");
    let assessment = read_input(&cwd.join(assessment_path));
    let decided = plan_decision::decide(&Inputs {
        bundle: &bundle,
        issue,
        verdict: as_input(&verdict),
        assessment_path,
        assessment: as_input(&assessment),
        artifact: args.required("--assessment-artifact"),
        harness: args.required("--harness"),
        model: args.required("--model"),
    })?;
    write_both(&[(cwd.join(RECORD), decided.text())])
        .map_err(|_| Unusable::new(format!("cannot write the record {RECORD}")))?;

    let record = decided.record();
    let floor = match (record.layers.floor.status, record.layers.floor.verdict) {
        (LayerStatus::Usable, Some(RuleVerdict::Hit)) => "hit",
        (LayerStatus::Usable, Some(RuleVerdict::Clean)) => "clean",
        _ => "unusable",
    };
    let judgement = &record.layers.judgement;
    let judged_as = match (judgement.status, judgement.band, judgement.confidence) {
        (LayerStatus::Usable, Some(band), Some(confidence)) => {
            format!("{} {confidence}", band_name(band))
        }
        _ => "unusable".to_owned(),
    };
    let line = format!(
        "decision: {} (mode {}, floor {floor}, judgement {judged_as}, judged {})",
        decision_name(record.decision),
        mode_name(record.mode),
        decision_name(record.judged_decision),
    );
    Ok((
        line,
        decided.comment().map(str::to_owned),
        decided.outcome(),
    ))
}

/// A file the decision reads, as `decision.py` reads it: missing when it does not exist,
/// unreadable on any other error, a directory included.
enum Read {
    Missing,
    Unreadable,
    Bytes(Vec<u8>),
}

fn read_input(path: &Path) -> Read {
    match std::fs::read(path) {
        Ok(bytes) => Read::Bytes(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => Read::Missing,
        Err(_) => Read::Unreadable,
    }
}

fn as_input(read: &Read) -> Input<'_> {
    match read {
        Read::Missing => Input::Missing,
        Read::Unreadable => Input::Unreadable,
        Read::Bytes(bytes) => Input::Bytes(bytes),
    }
}

fn decision_name(decision: Decision) -> &'static str {
    match decision {
        Decision::Auto => "auto",
        Decision::Human => "human",
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Always => "always",
        Mode::Shadow => "shadow",
        Mode::Judged => "judged",
        Mode::Never => "never",
    }
}

fn band_name(band: Band) -> &'static str {
    match band {
        Band::Safe => "safe",
        Band::LikelySafe => "likely-safe",
        Band::NeedsReview => "needs-review",
        Band::Uncertain => "uncertain",
        Band::Blocked => "blocked",
    }
}
