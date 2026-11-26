//! The command reader, its hints, the Commands section and the package reader, with the parsing
//! cases of `.mintswarm/dogfood/github/watch-plan-approvals.py`'s `command_of` translated to the
//! `approve`, `reject` and `help` words.

use std::fs;
use std::path::Path;

use mintswarm_domain::records::{DecisionRecord, FloorRecord};
use mintswarm_rules::binding::digest;
use mintswarm_rules::command::{Command, Hint, PackageKind, PackageRead, Reading, package, read};
use mintswarm_rules::render::approval::{Package, approval_package};
use mintswarm_rules::render::commands;
use serde_json::Value;

fn command(body: &str) -> Option<(Command, String)> {
    match read(body) {
        Some(Reading::Command { command, note }) => Some((command, note)),
        _ => None,
    }
}

fn hint(body: &str) -> Option<Hint> {
    match read(body) {
        Some(Reading::Hint(hint)) => Some(hint),
        _ => None,
    }
}

#[test]
fn command_lines_parse_with_their_note() {
    let cases = [
        ("Approve", Command::Approve),
        ("APPROVE", Command::Approve),
        ("reject", Command::Reject),
        ("help", Command::Help),
    ];
    for (word, expected) in cases {
        let body = format!("\r\nLooks good.\r\n  \t{word} \t\r\nShip it.\r\n\r\n");
        assert_eq!(
            command(&body),
            Some((expected, "Looks good.\nShip it.".to_owned())),
            "{word:?}"
        );
    }
}

#[test]
fn a_command_with_a_crlf_note() {
    assert_eq!(
        command("approve \r\nlooks good"),
        Some((Command::Approve, "looks good".to_owned()))
    );
    assert_eq!(command("help"), Some((Command::Help, String::new())));
}

#[test]
fn the_note_keeps_its_inner_lines_verbatim() {
    assert_eq!(
        command("First\n\n  indented\napprove\n> quoted\n\nlast  \n\n"),
        Some((
            Command::Approve,
            "First\n\n  indented\n> quoted\n\nlast  ".to_owned()
        ))
    );
}

#[test]
fn quoted_and_code_lines_are_not_commands() {
    let none = [
        "Should I post\n> approve\nhere?",
        "> Should I post\napprove",
        ">> nested\napprove",
        "Post this:\n```\napprove\n```",
        "Post this:\n~~~text\napprove\n~~~",
        "Post this:\n```\napprove",
        "Post this:\n\n    approve",
        "\tapprove",
    ];
    for body in none {
        assert_eq!(read(body), None, "{body:?}");
    }
    let some = [
        "> Should I post this?\n\napprove",
        "```\nreject\n```\napprove",
        ">\napprove",
        "> quoted\n```\n```\napprove",
        "   approve",
    ];
    for body in some {
        assert_eq!(
            command(body).map(|(command, _)| command),
            Some(Command::Approve),
            "{body:?}"
        );
    }
}

#[test]
fn a_fence_closes_only_on_a_long_enough_run() {
    assert_eq!(read("````\n```\napprove\n```"), None);
    assert_eq!(
        command("```\nhelp\n`````\napprove").map(|(command, _)| command),
        Some(Command::Approve)
    );
}

#[test]
fn a_command_after_quoted_text_counts() {
    assert_eq!(
        command("Looks good to me.\napprove"),
        Some((Command::Approve, "Looks good to me.".to_owned()))
    );
}

#[test]
fn first_command_line_wins() {
    assert_eq!(
        command("reject\nwhy not\napprove"),
        Some((Command::Reject, "why not\napprove".to_owned()))
    );
    assert_eq!(
        command("help\napprove").map(|(command, _)| command),
        Some(Command::Help)
    );
}

#[test]
fn a_command_line_beats_a_hint_line() {
    assert_eq!(
        command("approved\n/approve-plan\nreject").map(|(command, _)| command),
        Some(Command::Reject)
    );
}

#[test]
fn near_misses_get_hints() {
    let not_a_command = hint("/approve-plan").expect("a hint");
    assert_eq!(not_a_command, Hint::NotACommand("/approve-plan".to_owned()));
    assert_eq!(
        not_a_command.to_string(),
        "`/approve-plan` is not a command"
    );
    let approved = hint("approved").expect("a hint");
    assert_eq!(approved, Hint::DidYouMean(Command::Approve));
    assert_eq!(approved.to_string(), "did you mean `approve`?");
    assert_eq!(hint("rejct"), Some(Hint::DidYouMean(Command::Reject)));
    assert_eq!(hint("Aprove"), Some(Hint::DidYouMean(Command::Approve)));
    assert_eq!(
        hint("  /Help  "),
        Some(Hint::NotACommand("/Help".to_owned()))
    );
    assert_eq!(
        hint("Thanks\nrejct\napproved"),
        Some(Hint::DidYouMean(Command::Reject))
    );
}

#[test]
fn hidden_and_mixed_near_misses_get_nothing() {
    for body in [
        "> approved",
        "```\nrejct\n```",
        "    /approve-plan",
        "Approve.",
        "approve please",
        "/approve-plan please",
    ] {
        assert_eq!(read(body), None, "{body:?}");
    }
}

#[test]
fn session_and_prose_words_get_nothing() {
    for word in [
        "changes",
        "veto",
        "steer",
        "continue",
        "accept",
        "replan",
        "cancel",
        "planning",
        "This looks fine to me, thanks.",
    ] {
        assert_eq!(read(word), None, "{word:?}");
    }
}

/// The backticked word opening each row of a Commands section.
fn listed_words(section: &str) -> Vec<&str> {
    section
        .lines()
        .filter_map(|line| line.strip_prefix("- `"))
        .filter_map(|rest| rest.split('`').next())
        .collect()
}

#[test]
fn commands_section_lists_only_parsable_commands() {
    for kind in PackageKind::ALL {
        let section = commands(kind);
        assert!(section.starts_with("<details><summary>Commands</summary>\n\n"));
        assert!(section.ends_with("\n\n</details>\n"));
        assert!(
            section.contains(&format!(
                "the {} in the latest approval package",
                kind.noun()
            )),
            "{kind:?}"
        );
        let words = listed_words(&section);
        let parsed: Vec<Command> = words
            .iter()
            .map(|word| command(word).expect("a listed word parses").0)
            .collect();
        assert_eq!(parsed, Command::ALL, "{kind:?}");
    }
}

#[test]
fn the_plan_commands_section_reads_in_full() {
    assert_eq!(
        commands(PackageKind::Plan),
        "<details><summary>Commands</summary>\n\
         \n\
         - `approve`: approve the plan in the latest approval package; the comment's other lines \
         are a note.\n\
         - `reject`: reject the plan in the latest approval package; the comment's other lines are \
         a note.\n\
         - `help`: reply with the commands valid on this issue now.\n\
         \n\
         A command counts only when it stands alone on its own line, outside quotes and code; only \
         the first one in a comment counts.\n\
         \n\
         </details>\n"
    );
}

#[test]
fn package_reader() {
    let sha = "0123456789abcdef".repeat(4);
    for kind in PackageKind::ALL {
        let body = format!(
            "  {}  \r\n {} \r\n\r\n## Approval package\r\n",
            kind.marker(),
            kind.digest_line(&sha)
        );
        assert_eq!(
            package(&body),
            Some(PackageRead::Current {
                kind,
                sha256: sha.clone()
            }),
            "{kind:?}"
        );
        let not_current = |second: &str| {
            let body = format!("{}\n{second}", kind.marker());
            assert_eq!(
                package(&body),
                Some(PackageRead::NotCurrent(kind)),
                "{kind:?} {second:?}"
            );
        };
        not_current(&kind.digest_line(&sha[..63]));
        not_current(&kind.digest_line(&sha.to_uppercase()));
        not_current(&kind.digest_line(&format!("{}g", &sha[..63])));
        not_current(&kind.digest_line(&format!("{sha}0")));
        not_current("");
        not_current(&format!("{} sha256:  {sha}", kind.label()));
        let other = PackageKind::ALL
            .into_iter()
            .find(|other| *other != kind)
            .expect("another kind");
        not_current(&other.digest_line(&sha));
        assert_eq!(
            package(kind.marker()),
            Some(PackageRead::NotCurrent(kind)),
            "{kind:?} without a second line"
        );
    }
    assert_eq!(package(&format!("Plan sha256: {sha}")), None);
    assert_eq!(
        package(&format!(
            "Hello\n{}\nPlan sha256: {sha}",
            PackageKind::Plan.marker()
        )),
        None
    );
    assert_eq!(package(""), None);
}

#[test]
fn approval_package_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plan_session.jsonl");
    let fixture = fs::read_to_string(path).expect("the fixture");
    let job: Value =
        serde_json::from_str(fixture.lines().next().expect("a job")).expect("a JSON job");
    let text = |key: &str| job[key].as_str().expect("a string");
    let record: DecisionRecord = serde_json::from_str(text("decision")).expect("a decision");
    let floor: FloorRecord = serde_json::from_str(text("floor")).expect("a floor record");
    let plan = text("plan");
    let section = commands(PackageKind::Plan);
    let body = approval_package(&Package {
        plan: plan.as_bytes(),
        tldr_url: "https://example.invalid/tldr",
        plan_url: "https://example.invalid/plan",
        note: "",
        size: &[],
        floor: &floor,
        packages: &[],
        decision: &record,
        assessment_url: "https://example.invalid/assessment",
        decision_url: "https://example.invalid/decision",
        shadow_url: None,
        commands: Some(&section),
    });
    assert_eq!(
        package(&body),
        Some(PackageRead::Current {
            kind: PackageKind::Plan,
            sha256: digest(plan.as_bytes())
        })
    );
    assert_eq!(read(&body), None, "a package gives no command");
}
