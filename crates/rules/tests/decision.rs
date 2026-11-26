//! The records of design #548 §7: their first lines, the markers read back from them, the
//! confinement of user text in them, the Commands section in help and hints, and the shadow
//! decision line in the session's shadow records (`tests/fixtures/plan_session.jsonl`).

use std::fs;
use std::path::Path;

use mintswarm_domain::ids::RevId;
use mintswarm_domain::records::Decision;
use mintswarm_domain::tracker::{CheckState, Login, Review};
use mintswarm_rules::command::{self, Command, Hint, PackageKind};
use mintswarm_rules::markdown::scan_fences;
use mintswarm_rules::render::commands;
use mintswarm_rules::render::decision::{
    Marker, PENDING_DECISION, Record, Ruling, agrees, answer_of, decide_shadow, help, hint,
    merge_ready, merge_refused, pr_hint, read, record, refusal, set_back, shadow_line,
};
use serde_json::Value;

const SHA256: &str = "9f86d081884c7d659a2feb1b0b39dd0dc1b5ce1e5a0a6a64c6b4b6a3d1bdf7a8";
const HEAD40: &str = "0123456789abcdef0123456789abcdef01234567";
const STEER: &str = "<!-- mintswarm: steer -->";

fn head64() -> String {
    format!("{HEAD40}{}", &SHA256[..24])
}

fn login(name: &str) -> Login {
    Login(name.to_owned())
}

fn decision_record(kind: PackageKind, ruling: Ruling, answer: Option<u64>, note: &str) -> String {
    let approver = login("zmij");
    record(&Record {
        kind,
        ruling,
        answer,
        approver: &approver,
        time: "2026-11-26T10:00:00Z",
        package_url: "https://github.com/o/r/issues/5#issuecomment-77",
        sha256: SHA256,
        note,
    })
}

fn review(name: &str) -> Review {
    Review {
        id: 1,
        author: login(name),
        state: "APPROVED".to_owned(),
        submitted_at: Some("2026-11-26T10:00:00Z".to_owned()),
        commit: Some(RevId(HEAD40.to_owned())),
        url: "https://github.com/o/r/pull/9#pullrequestreview-1".to_owned(),
    }
}

fn check(name: &str, conclusion: Option<&str>) -> CheckState {
    CheckState {
        name: name.to_owned(),
        conclusion: conclusion.map(str::to_owned),
        source: "check-run".to_owned(),
        app: None,
    }
}

/// The first `n` lines of `body`, each with its LF.
fn opening(body: &str, n: usize) -> String {
    body.split_inclusive('\n').take(n).collect()
}

/// Every body of a kind other than a decision record, with the marker it opens with and its
/// literal first line.
fn other_bodies() -> Vec<(String, Marker, String)> {
    let h64 = head64();
    let mut bodies = vec![
        (
            refusal(41, "no package"),
            Marker::Answer(41),
            "<!-- mintswarm: answer 41 -->\n".to_owned(),
        ),
        (
            help(42, Some(PackageKind::Plan)),
            Marker::Answer(42),
            "<!-- mintswarm: answer 42 -->\n".to_owned(),
        ),
        (
            help(43, None),
            Marker::Answer(43),
            "<!-- mintswarm: answer 43 -->\n".to_owned(),
        ),
        (
            hint(44, &Hint::DidYouMean(Command::Approve), None),
            Marker::Answer(44),
            "<!-- mintswarm: answer 44 -->\n".to_owned(),
        ),
        (
            pr_hint(45),
            Marker::Answer(45),
            "<!-- mintswarm: answer 45 -->\n".to_owned(),
        ),
        (
            set_back(46, "set by hand"),
            Marker::SetBack(46),
            "<!-- mintswarm: set-back 46 -->\n".to_owned(),
        ),
        (
            merge_ready(&RevId(HEAD40.to_owned()), &[review("zmij")], &[]),
            Marker::MergeReady,
            "<!-- mintswarm: merge ready -->\n".to_owned(),
        ),
    ];
    for head in [HEAD40.to_owned(), h64] {
        bodies.push((
            merge_refused(&RevId(head.clone()), &["dirty"]),
            Marker::MergeRefused(RevId(head.clone())),
            format!("<!-- mintswarm: merge refused {head} -->\n"),
        ));
    }
    bodies
}

#[test]
fn every_record_renders_its_first_lines_and_parses_back() {
    for kind in PackageKind::ALL {
        for ruling in Ruling::ALL {
            for answer in [None, Some(7_u64), Some(u64::MAX)] {
                let body = decision_record(kind, ruling, answer, "a note");
                let mut expected = format!(
                    "<!-- mintswarm: {} {} -->\n<!-- mintswarm: steer -->\n",
                    kind.noun(),
                    ruling.word()
                );
                if let Some(id) = answer {
                    expected.push_str(&format!("<!-- mintswarm: answer {id} -->\n"));
                }
                let count = if answer.is_some() { 3 } else { 2 };
                assert_eq!(opening(&body, count), expected);
                let marker = Marker::Decision {
                    kind,
                    ruling,
                    answer,
                };
                assert_eq!(marker.lines(), expected);
                assert_eq!(read(&body), Some(marker));
                assert_eq!(read(&body.replace('\n', "\r\n")), read(&body));
                assert_eq!(answer_of(&body), answer);
            }
        }
    }
    for (body, marker, first) in other_bodies() {
        assert_eq!(opening(&body, 1), first);
        assert_eq!(marker.lines(), first);
        assert_eq!(read(&body).as_ref(), Some(&marker), "{body}");
        let answer = match marker {
            Marker::Answer(id) => Some(id),
            _ => None,
        };
        assert_eq!(answer_of(&body), answer);
    }
}

#[test]
fn record_fields_follow_the_markers() {
    let body = decision_record(PackageKind::Design, Ruling::Rejected, Some(7), "");
    assert_eq!(
        body,
        format!(
            "<!-- mintswarm: design rejected -->\n{STEER}\n<!-- mintswarm: answer 7 -->\n\n\
             **Design rejected** by `zmij` at `2026-11-26T10:00:00Z`.\n\n\
             - Package: https://github.com/o/r/issues/5#issuecomment-77\n\
             - Design sha256: {SHA256}\n\nNote: none.\n"
        )
    );
    let noted = decision_record(
        PackageKind::Plan,
        Ruling::Approved,
        None,
        "line one\r\nline two",
    );
    assert!(
        noted.ends_with("\n\nNote:\n\n```\nline one\nline two\n```\n"),
        "{noted}"
    );
}

#[test]
fn the_plan_approval_record_opens_as_today() {
    for answer in [None, Some(12)] {
        let body = decision_record(PackageKind::Plan, Ruling::Approved, answer, "");
        assert_eq!(
            body.split('\n').next(),
            Some("<!-- mintswarm: plan approved -->")
        );
    }
}

/// The lines of `body` outside its fenced blocks, without the code spans in them.
fn outside_code(body: &str) -> Vec<String> {
    let lines: Vec<&str> = body.split('\n').collect();
    scan_fences(&lines)
        .outside
        .into_iter()
        .map(|(_, line)| line.split('`').step_by(2).collect::<Vec<_>>().concat())
        .collect()
}

/// That no `@` and no `<!--` line lies outside the opening `markers` lines, fences and code
/// spans of `body`.
fn assert_inert(body: &str, markers: usize) {
    let lines = outside_code(body);
    for line in &lines {
        assert!(!line.contains('@'), "a live mention in {line:?} of\n{body}");
    }
    let tail: Vec<&str> = body.split('\n').skip(markers).collect();
    let fences = scan_fences(&tail);
    for (_, line) in fences.outside {
        assert!(
            !line.starts_with("<!--"),
            "an extra marker {line:?} in\n{body}"
        );
    }
    assert_eq!(
        command::package(body),
        None,
        "a package marker leaks in\n{body}"
    );
}

#[test]
fn hostile_notes_stay_inert() {
    let mut note = String::from("ping @zmij and @mintswarm/owners\n");
    for run in 1..=5 {
        note.push_str(&format!("{} code {}\n", "`".repeat(run), "`".repeat(run)));
    }
    note.push_str("<!-- mintswarm: steer -->\n<!-- mintswarm: answer 9 -->\n");
    note.push_str("<!-- mintswarm: plan approved -->\n<!-- mintswarm: plan approval package -->\n");
    note.push_str("```\nan inner fence\n```\nlast @line");
    let approver = login("@evil`<!-- mintswarm: answer 1 -->");
    for answer in [None, Some(3)] {
        let body = record(&Record {
            kind: PackageKind::Plan,
            ruling: Ruling::Approved,
            answer,
            approver: &approver,
            time: "`@now`",
            package_url: "https://github.com/o/r/issues/5#issuecomment-77",
            sha256: SHA256,
            note: &note,
        });
        let markers = if answer.is_some() { 3 } else { 2 };
        assert_eq!(
            read(&body),
            Some(Marker::Decision {
                kind: PackageKind::Plan,
                ruling: Ruling::Approved,
                answer,
            })
        );
        assert_inert(&body, markers);
        let lines: Vec<&str> = body.split('\n').collect();
        let blocks = scan_fences(&lines).blocks;
        assert_eq!(blocks.len(), 1, "{body}");
        assert_eq!(blocks[0].content.join("\n"), note);
        let fence = lines
            .iter()
            .find(|line| line.starts_with("```"))
            .expect("a fence");
        assert_eq!(fence.len(), 6);
    }
    let hostile = "@zmij `x` <!-- mintswarm: steer -->\n<!-- mintswarm: answer 2 -->";
    let mut reasons: Vec<(String, usize)> = vec![
        (refusal(1, hostile), 1),
        (set_back(2, hostile), 1),
        (
            merge_refused(&RevId(HEAD40.to_owned()), &[hostile, hostile]),
            1,
        ),
        (
            merge_ready(
                &RevId(HEAD40.to_owned()),
                &[review(hostile)],
                &[check(hostile, Some(hostile)), check("ci", None)],
            ),
            1,
        ),
    ];
    // The refusal marker carries the head as given; past it, the head is in a span.
    let hostile_head = RevId(hostile.to_owned());
    for body in [
        merge_refused(&hostile_head, &[hostile]),
        merge_ready(&hostile_head, &[], &[]),
    ] {
        let (_, rest) = body.split_once(" -->\n\n").expect("a marker line");
        assert_inert(rest, 0);
    }
    let hostile_hint = Hint::NotACommand(hostile.to_owned());
    reasons.push((hint(4, &hostile_hint, Some(PackageKind::Plan)), 1));
    reasons.push((hint(4, &hostile_hint, None), 1));
    for (body, markers) in reasons {
        assert_inert(&body, markers);
        assert!(!body.contains("<!-- mintswarm: answer 2 -->\n"), "{body}");
    }
}

#[test]
fn help_and_hint_embed_the_commands_section() {
    let hints = [
        Hint::DidYouMean(Command::Reject),
        Hint::NotACommand("/approve".to_owned()),
    ];
    for kind in PackageKind::ALL {
        let section = commands(kind);
        let others: Vec<String> = PackageKind::ALL
            .into_iter()
            .filter(|other| *other != kind)
            .map(commands)
            .collect();
        let mut bodies = vec![help(1, Some(kind))];
        bodies.extend(hints.iter().map(|h| hint(1, h, Some(kind))));
        for body in bodies {
            assert!(body.contains(&section), "{body}");
            for other in &others {
                assert!(!body.contains(other.as_str()), "{body}");
            }
        }
    }
    let mut bodies = vec![help(1, None)];
    bodies.extend(hints.iter().map(|h| hint(1, h, None)));
    for body in bodies {
        assert!(!body.contains("<summary>Commands</summary>"), "{body}");
        assert!(body.contains("no command applies now"), "{body}");
    }
    assert!(hint(1, &hints[0], None).contains("\nDid you mean `reject`?\n"));
    assert!(hint(1, &hints[1], None).contains("\n`/approve` is not a command.\n"));
}

#[test]
fn pr_bodies_carry_no_steer_line() {
    let head = RevId(HEAD40.to_owned());
    for body in [
        merge_ready(
            &head,
            &[review("zmij")],
            &[check("gates-matrix", Some("success"))],
        ),
        merge_refused(&head, &["dirty", "checks pending"]),
        pr_hint(5),
    ] {
        assert!(!body.split('\n').any(|line| line == STEER), "{body}");
    }
}

#[test]
fn merge_records_list_reviews_checks_and_reasons() {
    let head = RevId(HEAD40.to_owned());
    assert_eq!(
        merge_ready(
            &head,
            &[review("zmij")],
            &[check("gates-matrix", Some("success")), check("ci", None)]
        ),
        format!(
            "<!-- mintswarm: merge ready -->\n\nHead: `{HEAD40}`\n\n\
             - Approved by `zmij` ([review](https://github.com/o/r/pull/9#pullrequestreview-1))\n\
             - Check `gates-matrix`: `success`\n- Check `ci`: `pending`\n"
        )
    );
    assert_eq!(
        merge_refused(&head, &["dirty", "a review thread is unresolved"]),
        format!(
            "<!-- mintswarm: merge refused {HEAD40} -->\n\nNot merged at head `{HEAD40}`:\n\n\
             - `dirty`\n- `a review thread is unresolved`\n"
        )
    );
}

#[test]
fn the_set_back_points_at_approve() {
    let body = set_back(46, "set by hand");
    assert!(body.contains("`awaiting approval`"), "{body}");
    assert!(body.contains("`set by hand`"), "{body}");
    assert!(
        body.contains(&format!("comment `{}`", Command::Approve.word())),
        "{body}"
    );
    assert!(!body.split('\n').any(|line| line == STEER), "{body}");
}

#[test]
fn agreement_follows_judged_decision() {
    assert!(agrees(Ruling::Approved, Decision::Auto));
    assert!(!agrees(Ruling::Approved, Decision::Human));
    assert!(agrees(Ruling::Rejected, Decision::Human));
    assert!(!agrees(Ruling::Rejected, Decision::Auto));
    assert_eq!(
        shadow_line(Ruling::Approved, Decision::Auto, "https://x.invalid/r"),
        "- **Maintainer decision:** approved · agrees with judged_decision: yes · \
         [record](https://x.invalid/r)"
    );
    assert_eq!(
        shadow_line(Ruling::Rejected, Decision::Auto, "https://x.invalid/r"),
        "- **Maintainer decision:** rejected · agrees with judged_decision: no · \
         [record](https://x.invalid/r)"
    );
}

fn shadow_records() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plan_session.jsonl");
    fs::read_to_string(path)
        .expect("the fixture")
        .lines()
        .map(|line| {
            let job: Value = serde_json::from_str(line).expect("a fixture line is JSON");
            job["shadow_record"]
                .as_str()
                .expect("a shadow record")
                .to_owned()
        })
        .collect()
}

#[test]
fn the_shadow_line_replaces_pending_in_session_records() {
    let records = shadow_records();
    assert_eq!(records.len(), 3);
    let url = "https://github.com/o/r/issues/5#issuecomment-99";
    for shadow in &records {
        let start = shadow
            .find(&format!("\n{PENDING_DECISION}\n"))
            .expect("a pending line")
            + 1;
        let end = start + PENDING_DECISION.len();
        for ruling in Ruling::ALL {
            for judged in [Decision::Auto, Decision::Human] {
                let line = shadow_line(ruling, judged, url);
                let decided = decide_shadow(shadow, ruling, judged, url).expect("decided");
                assert_eq!(decided[..start], shadow[..start]);
                assert_eq!(decided[start..start + line.len()], line);
                assert_eq!(decided[start + line.len()..], shadow[end..]);
                assert_eq!(decide_shadow(&decided, ruling, judged, url), None);
            }
        }
        let crlf = shadow.replace('\n', "\r\n");
        let decided = decide_shadow(&crlf, Ruling::Approved, Decision::Auto, url).expect("CRLF");
        let line = shadow_line(Ruling::Approved, Decision::Auto, url);
        assert_eq!(
            decided,
            crlf.replace(&format!("{PENDING_DECISION}\r\n"), &format!("{line}\r\n"))
        );
    }
    let twice = format!("{PENDING_DECISION}\n{PENDING_DECISION}\n");
    assert_eq!(
        decide_shadow(&twice, Ruling::Approved, Decision::Auto, url),
        None
    );
    assert_eq!(
        decide_shadow("no row\n", Ruling::Approved, Decision::Auto, url),
        None
    );
    let indented = format!(" {PENDING_DECISION}\n");
    assert_eq!(
        decide_shadow(&indented, Ruling::Approved, Decision::Auto, url),
        None
    );
}
