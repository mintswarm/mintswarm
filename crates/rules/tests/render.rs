//! The plan-approval bodies: the confinement of model text against `decision.py`'s `inert` and
//! `span` (captured in `tests/fixtures/plan_decision.jsonl`) and `check_span`, the TL;DR check,
//! and the TL;DR comment, shadow record and approval package of session jobs, against the bodies
//! the session posted (`tests/fixtures/plan_session.jsonl`).

use std::fs;
use std::path::{Path, PathBuf};

use mintswarm_domain::records::{DecisionRecord, FloorRecord};
use mintswarm_rules::json::{self, Style};
use mintswarm_rules::plan_floor::footprint;
use mintswarm_rules::render::approval::{
    Artifacts, Package, TldrError, approval_package, shadow_record, tldr, tldr_comment,
};
use mintswarm_rules::render::comment::{package_rows, shadow_comment};
use mintswarm_rules::render::{SPAN_LIMIT, SPAN_MARK, inert, is_removed, span};
use serde_json::Value;

fn fixture(name: &str) -> Vec<Value> {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read_to_string(path)
        .expect("the fixture")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a fixture line is JSON"))
        .collect()
}

fn text<'v>(value: &'v Value, key: &str) -> &'v str {
    value[key].as_str().expect("a string")
}

#[test]
fn check_span() {
    assert_eq!((SPAN_LIMIT, SPAN_MARK), (600, " … (truncated)"));
    let under = "a".repeat(SPAN_LIMIT - 1);
    assert_eq!(span(&under), format!("`{under}`"));
    let at = "b".repeat(SPAN_LIMIT);
    assert_eq!(span(&at), format!("`{at}`"));
    assert_eq!(span(&format!("  \n{at}\n  ")), format!("`{at}`"));
    let over = format!("{}d", "c".repeat(SPAN_LIMIT));
    assert_eq!(
        span(&over),
        format!("`{}{SPAN_MARK}`", "c".repeat(SPAN_LIMIT))
    );
    let spaced = format!("{} fff", "e".repeat(SPAN_LIMIT - 1));
    assert_eq!(
        span(&spaced),
        format!("`{}{SPAN_MARK}`", "e".repeat(SPAN_LIMIT - 1))
    );
    let hostile = format!("{}tail", "`a`\r\nb\u{2028}c\u{200b}d\u{202e} ".repeat(100));
    let out = span(&hostile);
    assert!(!out.contains('\n') && !out.contains('\r'));
    assert_eq!(out.matches('`').count(), 2);
    assert!(out.starts_with('`') && out.ends_with(&format!("{SPAN_MARK}`")));
    assert!(out.chars().count() <= SPAN_LIMIT + SPAN_MARK.chars().count() + 2);
    assert!(!out.chars().any(is_removed));
    assert_eq!(span(" \n "), "`(empty)`");
}

#[test]
fn inert_and_span_match_the_capture() {
    let lines = fixture("plan_decision.jsonl");
    let removed = lines
        .iter()
        .find(|line| line["kind"] == "inert")
        .expect("the inert line");
    let ranges: Vec<(u32, u32)> = removed["removed"]
        .as_array()
        .expect("ranges")
        .iter()
        .map(|r| {
            let bound =
                |i: usize| u32::try_from(r[i].as_u64().expect("a code point")).expect("u32");
            (bound(0), bound(1))
        })
        .collect();
    for c in (0..=0x10ffff).filter_map(char::from_u32) {
        let code = u32::from(c);
        let captured = ranges
            .iter()
            .any(|(low, high)| (*low..=*high).contains(&code));
        assert_eq!(
            is_removed(c),
            captured,
            "U+{code:04X} under Unicode {}",
            removed["unicode"]
        );
    }
    let corpus: Vec<&Value> = lines.iter().filter(|line| line["kind"] == "span").collect();
    assert!(corpus.len() >= 10);
    for case in corpus {
        let source = text(case, "text");
        assert_eq!(inert(source), text(case, "inert"), "inert of {source:?}");
        assert_eq!(span(source), text(case, "span"), "span of {source:?}");
    }
}

/// The shadow record and approval package of a session job.
fn session_bodies(job: &Value) -> (String, String) {
    let urls = &job["urls"];
    let url = |key: &str| urls[key].as_str().expect("a URL");
    let record: DecisionRecord =
        serde_json::from_str(text(job, "decision")).expect("a decision record");
    let floor: FloorRecord = serde_json::from_str(text(job, "floor")).expect("a floor record");
    let plan = text(job, "plan");
    let footprint = footprint::parse(plan).ok();
    let packages = package_rows(None, footprint.as_ref());
    let comment = shadow_comment(&record, footprint.as_ref(), &packages);
    let shadow = shadow_record(
        &comment,
        &Artifacts {
            plan: url("plan"),
            assessment: url("assessment"),
            decision: url("decision"),
        },
    );
    let package = approval_package(&Package {
        plan: plan.as_bytes(),
        tldr_url: url("tldr"),
        plan_url: url("plan"),
        note: text(job, "note"),
        size: &[],
        floor: &floor,
        packages: &[],
        decision: &record,
        assessment_url: url("assessment"),
        decision_url: url("decision"),
        shadow_url: Some(url("shadow")),
        commands: None,
    });
    (shadow, package)
}

#[test]
fn session_bodies_match_the_posted_ones() {
    let jobs = fixture("plan_session.jsonl");
    assert_eq!(jobs.len(), 3);
    for job in &jobs {
        let issue = &job["issue"];
        let plan = text(job, "plan");
        let tldr_body = tldr_comment(plan, job["urls"]["plan"].as_str().expect("a URL"));
        assert_eq!(
            tldr_body,
            text(job, "tldr_comment"),
            "#{issue} TL;DR comment"
        );
        let (shadow, package) = session_bodies(job);
        assert_eq!(shadow, text(job, "shadow_record"), "#{issue} shadow record");
        assert_eq!(
            package,
            text(job, "approval_package"),
            "#{issue} approval package"
        );
    }
}

#[test]
fn the_package_lists_the_authority_triggers() {
    let job = &fixture("plan_session.jsonl")[0];
    let urls = &job["urls"];
    let url = |key: &str| urls[key].as_str().expect("a URL");
    let mut record: DecisionRecord =
        serde_json::from_str(text(job, "decision")).expect("a decision record");
    let floor: FloorRecord = serde_json::from_str(text(job, "floor")).expect("a floor record");
    let confidence_row = |record: &DecisionRecord| -> String {
        let package = approval_package(&Package {
            plan: text(job, "plan").as_bytes(),
            tldr_url: url("tldr"),
            plan_url: url("plan"),
            note: text(job, "note"),
            size: &[],
            floor: &floor,
            packages: &[],
            decision: record,
            assessment_url: url("assessment"),
            decision_url: url("decision"),
            shadow_url: Some(url("shadow")),
            commands: None,
        });
        package
            .lines()
            .find(|line| line.starts_with("- **Confidence: "))
            .expect("a Confidence row")
            .to_owned()
    };
    assert!(record.layers.judgement.hard_triggers.is_empty());
    let plain = confidence_row(&record);
    assert!(!plain.contains("requires the maintainer"), "{plain}");
    record.layers.judgement.hard_triggers = vec![
        "criterion-not-met".to_owned(),
        "weakens-safeguard".to_owned(),
        "credentials".to_owned(),
    ];
    let listed = confidence_row(&record);
    let (band, rest) = plain.split_once("** (").expect("the band closes the bold");
    assert_eq!(
        listed,
        format!("{band}** · requires the maintainer: `weakens-safeguard`, `credentials` ({rest}")
    );
}

#[test]
fn shadow_records_match_without_their_url_line() {
    for job in fixture("plan_session.jsonl") {
        let posted = text(&job, "shadow_record");
        let (shadow, _) = session_bodies(&job);
        let comment = shadow
            .strip_suffix('\n')
            .and_then(|s| s.rsplit_once('\n'))
            .map(|(head, _)| head)
            .expect("an Artifacts line");
        let posted_comment = posted
            .strip_suffix('\n')
            .and_then(|s| s.rsplit_once('\n'))
            .map(|(head, _)| head)
            .expect("a URL line");
        assert_eq!(comment, posted_comment, "#{}", job["issue"]);
    }
}

#[test]
fn session_decision_records_read_back_byte_for_byte() {
    for job in fixture("plan_session.jsonl") {
        let original = text(&job, "decision");
        let record: DecisionRecord = serde_json::from_str(original).expect("a decision record");
        let tree = json::to_json(&record).expect("serialisable");
        assert_eq!(
            json::to_file_text(&tree, Style::DISK),
            original,
            "#{}",
            job["issue"]
        );
    }
}

const PLAN_HEAD: &str = "# Plan: a change\n\nAdds a thing; size S.\n\n";

fn plan_with(tldr_lines: &str) -> String {
    format!("{PLAN_HEAD}## TL;DR\n{tldr_lines}## Approach\nSteps.\n")
}

const GOOD: &str = "- Changes: `crates/x`.\n- Approach: one line.\n\n- Left out: nothing.\n- Maintainer: approval only.\n";

#[test]
fn a_usable_tldr_is_posted_unchanged() {
    let plan = plan_with(GOOD);
    assert_eq!(
        tldr(&plan),
        Ok(vec![
            "- Changes: `crates/x`.",
            "- Approach: one line.",
            "- Left out: nothing.",
            "- Maintainer: approval only.",
        ])
    );
    let spans = "- Changes: `#12`, `a@b`, `<x>`, `[a](b)` and `GH-3`.\n- b\n- c\n";
    assert!(tldr(&plan_with(spans)).is_ok());
}

#[test]
fn every_tldr_condition_rejects() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let cases: Vec<(String, TldrError)> = vec![
        (
            format!("{PLAN_HEAD}## Approach\n## TL;DR\n{GOOD}"),
            TldrError::NotAfterSummary,
        ),
        (
            format!("# Plan: a change\n\nAdds a thing.\nMore prose.\n\n## TL;DR\n{GOOD}"),
            TldrError::NotAfterSummary,
        ),
        (
            format!("{PLAN_HEAD}No headings.\n"),
            TldrError::NotAfterSummary,
        ),
        (plan_with("- a\n- b\n"), TldrError::Count),
        (
            plan_with("- a\n- b\n- c\n- d\n- e\n- f\n- g\n"),
            TldrError::Count,
        ),
        (plan_with("- a\n  continued\n- c\n"), TldrError::NotABullet),
        (plan_with("- a\n  - nested\n- c\n"), TldrError::NotABullet),
        (plan_with("- a `b\n- b\n- c\n"), TldrError::UnpairedBacktick),
        (
            plan_with("- see `https://x.invalid`\n- b\n- c\n"),
            TldrError::Url,
        ),
        (
            plan_with("- see www.example.invalid\n- b\n- c\n"),
            TldrError::Url,
        ),
        (plan_with("- ask @someone\n- b\n- c\n"), TldrError::Markup),
        (plan_with("- a <b> tag\n- b\n- c\n"), TldrError::Markup),
        (plan_with("- a -> b\n- b\n- c\n"), TldrError::Markup),
        (plan_with("- [a](b)\n- b\n- c\n"), TldrError::Markup),
        (plan_with("- fixes #12\n- b\n- c\n"), TldrError::Reference),
        (
            plan_with("- fixes owner/repo#12\n- b\n- c\n"),
            TldrError::Reference,
        ),
        (plan_with("- fixes GH-12\n- b\n- c\n"), TldrError::Reference),
        (
            plan_with(&format!("- at {sha}\n- b\n- c\n")),
            TldrError::Sha,
        ),
    ];
    for (plan, expected) in cases {
        assert_eq!(tldr(&plan), Err(expected), "{plan:?}");
        let comment = tldr_comment(&plan, "https://example.invalid/plan");
        assert_eq!(
            comment,
            "### Plan TL;DR\n\nThe plan has no usable TL;DR.\n\nFull plan: \
             https://example.invalid/plan\n"
        );
    }
}
