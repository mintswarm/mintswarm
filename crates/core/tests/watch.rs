//! The watch module's shared rules: the filter evaluator, the cursor codec, `next_id` and
//! `Retention`.

#![cfg(test)]

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mintswarm_core::watch::{Cursor, Filter, Retention, WatchError, next_id};
use mintswarm_proto::watch::v1::{
    Actor, ActorClass, AttributeMatch, Forwarding, ItemRef, Party, PartyMatch, Subject, Verdict,
    WatchEvent, WatchFilter,
};
use tonic::Code;
use uuid::{Builder, Uuid, Variant};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|&v| v.to_owned()).collect()
}

/// A `github.webhook` comment on mintswarm/mintswarm#484 by the maintainer, unset verdict.
fn event() -> WatchEvent {
    WatchEvent {
        kind: "github.webhook".to_owned(),
        subject: Some(Subject {
            repository: "mintswarm/mintswarm".to_owned(),
            number: 484,
            ..Subject::default()
        }),
        actor: Some(Actor {
            login: "maintainer".to_owned(),
            class: ActorClass::Maintainer.into(),
            ..Actor::default()
        }),
        parties: vec![Party {
            login: "author".to_owned(),
            role: "author".to_owned(),
        }],
        attributes: HashMap::from([("event".to_owned(), "issue_comment".to_owned())]),
        ..WatchEvent::default()
    }
}

fn kinds(values: &[&str]) -> WatchFilter {
    WatchFilter {
        kinds: strings(values),
        ..WatchFilter::default()
    }
}

fn with_verdict(verdict: Verdict, opt_in: &str) -> WatchEvent {
    WatchEvent {
        forwarding: Some(Forwarding {
            verdict: verdict.into(),
            rule: "rule".to_owned(),
            opt_in: opt_in.to_owned(),
        }),
        ..event()
    }
}

#[test]
fn filter_matches_kinds_exactly_or_by_prefix() {
    let plane = WatchEvent {
        kind: "plane.job".to_owned(),
        ..event()
    };
    let cases: &[(&[&str], bool, bool)] = &[
        (&["github.*"], true, false),
        (&["github.webhook"], true, false),
        (&["github.web"], false, false),
        (&["plane.*", "github.webhook"], true, true),
    ];
    for (values, github, plane_job) in cases {
        let filter = Filter::new(kinds(values)).expect("valid filter");
        assert_eq!(
            filter.matches(&event()),
            *github,
            "{values:?} on github.webhook"
        );
        assert_eq!(
            filter.matches(&plane),
            *plane_job,
            "{values:?} on plane.job"
        );
    }
}

#[test]
fn filter_refuses_malformed_values() {
    let base = || kinds(&["github.*"]);
    let cases: Vec<(&str, WatchFilter)> = vec![
        ("empty kinds", WatchFilter::default()),
        ("bare wildcard", kinds(&["*"])),
        ("empty kind", kinds(&[""])),
        ("inner wildcard", kinds(&["git*.webhook"])),
        ("no name", kinds(&["github"])),
        ("upper case", kinds(&["GitHub.webhook"])),
        ("three segments", kinds(&["github.webhook.x"])),
        (
            "repository",
            WatchFilter {
                repositories: strings(&["mintswarm"]),
                ..base()
            },
        ),
        (
            "empty owner",
            WatchFilter {
                repositories: strings(&["/mintswarm"]),
                ..base()
            },
        ),
        (
            "item repository",
            WatchFilter {
                items: vec![ItemRef {
                    repository: String::new(),
                    number: 1,
                }],
                ..base()
            },
        ),
        (
            "item number",
            WatchFilter {
                items: vec![ItemRef {
                    repository: "a/b".to_owned(),
                    number: 0,
                }],
                ..base()
            },
        ),
        (
            "actor login",
            WatchFilter {
                actors: strings(&[""]),
                ..base()
            },
        ),
        (
            "party login",
            WatchFilter {
                parties: vec![PartyMatch {
                    login: String::new(),
                    roles: vec![],
                }],
                ..base()
            },
        ),
        (
            "party role",
            WatchFilter {
                parties: vec![PartyMatch {
                    login: "a".to_owned(),
                    roles: strings(&[""]),
                }],
                ..base()
            },
        ),
        (
            "unspecified class",
            WatchFilter {
                actor_classes: vec![ActorClass::Unspecified.into()],
                ..base()
            },
        ),
        (
            "unknown class",
            WatchFilter {
                actor_classes: vec![99],
                ..base()
            },
        ),
        (
            "attribute key",
            WatchFilter {
                attributes: vec![AttributeMatch {
                    key: String::new(),
                    values: vec![],
                }],
                ..base()
            },
        ),
        (
            "opt-in",
            WatchFilter {
                include_opt_in: strings(&[""]),
                ..base()
            },
        ),
    ];
    for (name, filter) in cases {
        let error = Filter::new(filter).expect_err(name);
        assert!(matches!(error, WatchError::Filter(_)), "{name}: {error:?}");
        assert_eq!(
            tonic::Status::from(error).code(),
            Code::InvalidArgument,
            "{name}"
        );
    }
}

#[test]
fn filter_ors_within_a_field_and_ands_across_fields() {
    let base = || kinds(&["github.*"]);
    let cases: Vec<(&str, WatchFilter, bool)> = vec![
        ("empty fields", base(), true),
        (
            "repository",
            WatchFilter {
                repositories: strings(&["x/y", "mintswarm/mintswarm"]),
                ..base()
            },
            true,
        ),
        (
            "other repository",
            WatchFilter {
                repositories: strings(&["x/y"]),
                ..base()
            },
            false,
        ),
        (
            "mixed case",
            WatchFilter {
                repositories: strings(&["MintSwarm/MintSwarm"]),
                actors: strings(&["Maintainer"]),
                ..base()
            },
            true,
        ),
        (
            "actors",
            WatchFilter {
                actors: strings(&["someone", "maintainer"]),
                ..base()
            },
            true,
        ),
        (
            "other actor",
            WatchFilter {
                actors: strings(&["someone"]),
                ..base()
            },
            false,
        ),
        (
            "and across",
            WatchFilter {
                repositories: strings(&["mintswarm/mintswarm"]),
                actors: strings(&["someone"]),
                ..base()
            },
            false,
        ),
        (
            "item",
            WatchFilter {
                items: vec![ItemRef {
                    repository: "MINTSWARM/mintswarm".to_owned(),
                    number: 484,
                }],
                ..base()
            },
            true,
        ),
        (
            "other item",
            WatchFilter {
                items: vec![ItemRef {
                    repository: "mintswarm/mintswarm".to_owned(),
                    number: 1,
                }],
                ..base()
            },
            false,
        ),
        (
            "classes",
            WatchFilter {
                actor_classes: vec![ActorClass::App.into(), ActorClass::Maintainer.into()],
                ..base()
            },
            true,
        ),
        (
            "other class",
            WatchFilter {
                actor_classes: vec![ActorClass::App.into()],
                ..base()
            },
            false,
        ),
        (
            "party any role",
            WatchFilter {
                parties: vec![PartyMatch {
                    login: "Author".to_owned(),
                    roles: vec![],
                }],
                ..base()
            },
            true,
        ),
        (
            "party role",
            WatchFilter {
                parties: vec![PartyMatch {
                    login: "author".to_owned(),
                    roles: strings(&["assignee", "author"]),
                }],
                ..base()
            },
            true,
        ),
        (
            "party other role",
            WatchFilter {
                parties: vec![PartyMatch {
                    login: "author".to_owned(),
                    roles: strings(&["assignee"]),
                }],
                ..base()
            },
            false,
        ),
        (
            "attribute any value",
            WatchFilter {
                attributes: vec![AttributeMatch {
                    key: "event".to_owned(),
                    values: vec![],
                }],
                ..base()
            },
            true,
        ),
        (
            "attribute value",
            WatchFilter {
                attributes: vec![AttributeMatch {
                    key: "event".to_owned(),
                    values: strings(&["push", "issue_comment"]),
                }],
                ..base()
            },
            true,
        ),
        (
            "attribute other value",
            WatchFilter {
                attributes: vec![AttributeMatch {
                    key: "event".to_owned(),
                    values: strings(&["push"]),
                }],
                ..base()
            },
            false,
        ),
        (
            "attribute missing key",
            WatchFilter {
                attributes: vec![AttributeMatch {
                    key: "action".to_owned(),
                    values: vec![],
                }],
                ..base()
            },
            false,
        ),
    ];
    for (name, filter, expected) in cases {
        let filter = Filter::new(filter).expect(name);
        assert_eq!(filter.matches(&event()), expected, "{name}");
    }
}

#[test]
fn filter_applies_phase_2_verdicts() {
    let base = || kinds(&["github.*"]);
    let opt_in = || WatchFilter {
        include_opt_in: strings(&["bots"]),
        ..base()
    };
    let dropped = || WatchFilter {
        include_dropped: true,
        ..base()
    };
    let unknown = WatchEvent {
        forwarding: Some(Forwarding {
            verdict: 99,
            ..Forwarding::default()
        }),
        ..event()
    };
    let cases: Vec<(&str, WatchFilter, WatchEvent, bool)> = vec![
        ("unset", base(), event(), true),
        ("pass", base(), with_verdict(Verdict::Pass, ""), true),
        (
            "unspecified",
            base(),
            with_verdict(Verdict::Unspecified, ""),
            true,
        ),
        ("unknown", base(), unknown, true),
        (
            "opt-in not included",
            base(),
            with_verdict(Verdict::OptIn, "bots"),
            false,
        ),
        (
            "opt-in included",
            opt_in(),
            with_verdict(Verdict::OptIn, "bots"),
            true,
        ),
        (
            "other opt-in",
            opt_in(),
            with_verdict(Verdict::OptIn, "noise"),
            false,
        ),
        ("drop", base(), with_verdict(Verdict::Drop, ""), false),
        (
            "drop included",
            dropped(),
            with_verdict(Verdict::Drop, ""),
            true,
        ),
    ];
    for (name, filter, event, expected) in cases {
        let filter = Filter::new(filter).expect(name);
        assert_eq!(filter.matches(&event), expected, "{name}");
    }
}

fn assert_v7(id: &Uuid) {
    assert_eq!(id.get_version_num(), 7, "{id}");
    assert_eq!(id.get_variant(), Variant::RFC4122, "{id}");
}

fn now_millis() -> u64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after the epoch");
    u64::try_from(since.as_millis()).expect("millis fit")
}

/// A UUIDv7 an hour ahead of the clock.
fn future_id() -> Uuid {
    Builder::from_unix_timestamp_millis(now_millis() + 3_600_000, &[0x5a; 10]).into_uuid()
}

const EPOCH: &str = "0193a1b2-0000-4000-8000-000000000001";
const ID: &str = "0193a1b2-0000-7000-8000-000000000002";

#[test]
fn cursor_round_trips() {
    let text = format!("{EPOCH}.{ID}");
    let cursor = Cursor::parse(&text).expect("valid cursor");
    assert_eq!(cursor.to_string(), text);
    assert_eq!(cursor.epoch(), Uuid::parse_str(EPOCH).expect("uuid"));
    assert_eq!(cursor.id(), Uuid::parse_str(ID).expect("uuid"));
    let upper = Cursor::parse(&text.to_uppercase()).expect("upper case");
    assert_eq!(upper.to_string(), text);
    let built = Cursor::new(cursor.epoch(), cursor.id()).expect("valid parts");
    assert_eq!(built, cursor);
}

#[test]
fn cursor_refuses_malformed_strings() {
    let simple = ID.replace('-', "");
    let v4 = "0193a1b2-0000-4000-8000-000000000002";
    let nil = Uuid::nil().to_string();
    let cases: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("no dot", format!("{EPOCH}{ID}")),
        ("empty epoch", format!(".{ID}")),
        ("empty id", format!("{EPOCH}.")),
        ("extra dot", format!("{EPOCH}.{ID}.{ID}")),
        ("simple", format!("{EPOCH}.{simple}")),
        ("braced", format!("{EPOCH}.{{{ID}}}")),
        ("urn", format!("{EPOCH}.urn:uuid:{ID}")),
        ("not hex", format!("{EPOCH}.{}", ID.replace('0', "g"))),
        ("v4 id", format!("{EPOCH}.{v4}")),
        ("nil epoch", format!("{nil}.{ID}")),
    ];
    for (name, text) in cases {
        let error = Cursor::parse(&text).expect_err(name);
        assert!(matches!(error, WatchError::Cursor(_)), "{name}: {error:?}");
        assert_eq!(
            tonic::Status::from(error).code(),
            Code::InvalidArgument,
            "{name}"
        );
    }
}

#[test]
fn next_id_without_latest_is_a_v7() {
    assert_v7(&next_id(None).expect("an id"));
}

#[test]
fn next_id_sorts_after_a_latest_from_a_later_millisecond() {
    let latest = future_id();
    let id = next_id(Some(latest)).expect("an id");
    assert!(id > latest, "{id} after {latest}");
    assert_v7(&id);
}

#[test]
fn next_id_carries_into_the_next_millisecond() {
    let millis = now_millis() + 3_600_000;
    let latest = Builder::from_unix_timestamp_millis(millis, &[0xff; 10]).into_uuid();
    let id = next_id(Some(latest)).expect("an id");
    assert_v7(&id);
    let next = Builder::from_unix_timestamp_millis(millis + 1, &[0; 10]).into_uuid();
    assert_eq!(id, next);
}

#[test]
fn next_id_increases_over_ten_thousand_calls() {
    for start in [Some(future_id()), None] {
        let mut latest = start;
        for _ in 0..10_000 {
            let id = next_id(latest).expect("an id");
            assert_v7(&id);
            if let Some(previous) = latest {
                assert!(id > previous, "{id} after {previous}");
            }
            latest = Some(id);
        }
    }
}

#[test]
fn next_id_refuses_a_latest_that_is_not_a_v7() {
    let v4 = Uuid::parse_str("0193a1b2-0000-4000-8000-000000000002").expect("uuid");
    let error = next_id(Some(v4)).expect_err("not a v7");
    assert!(matches!(error, WatchError::Id(_)), "{error:?}");
    assert_eq!(tonic::Status::from(error).code(), Code::Internal);
}

#[test]
fn next_id_refuses_when_no_id_follows() {
    let last = Builder::from_unix_timestamp_millis((1 << 48) - 1, &[0xff; 10]).into_uuid();
    assert!(matches!(next_id(Some(last)), Err(WatchError::Id(_))));
}

#[test]
fn retention_refuses_zero_bounds() {
    let day = Duration::from_secs(86_400);
    for (name, age, count) in [("zero age", Duration::ZERO, 10), ("zero count", day, 0)] {
        let error = Retention::new(age, count).expect_err(name);
        assert!(
            matches!(error, WatchError::Retention(_)),
            "{name}: {error:?}"
        );
    }
    assert!(
        Retention::new(Duration::MAX, 10).is_err(),
        "unrepresentable age"
    );
}

#[test]
fn retention_round_trips() {
    let age = Duration::new(86_400, 500);
    let retention = Retention::new(age, 10_000).expect("positive bounds");
    assert_eq!(retention.max_age(), age);
    assert_eq!(retention.max_events(), 10_000);
    let proto = retention.to_proto(format!("{EPOCH}.{ID}"));
    assert_eq!(proto.max_events, 10_000);
    assert_eq!(proto.oldest_cursor, format!("{EPOCH}.{ID}"));
    let max_age = proto.max_age.expect("max_age set");
    assert_eq!((max_age.seconds, max_age.nanos), (86_400, 500));
}
