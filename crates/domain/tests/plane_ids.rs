//! The plane identifiers and the watch-event type round-trip through JSON.

use std::fmt::Debug;

use mintswarm_domain::ids::{
    AccountNickname, DirectiveId, IssueNumber, LeaseEpoch, PrNumber, ProjectId, RevId, RunnerId,
};
use mintswarm_domain::tracker::{Login, Review};
use mintswarm_domain::watch::{
    CommandEvent, IssueChange, IssueChangedEvent, MergedEvent, ReviewEvent, WatchEvent,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn round_trip<T>(value: &T) -> Value
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let text = serde_json::to_string(value).expect("serialises");
    let back: T = serde_json::from_str(&text).expect("deserialises");
    assert_eq!(&back, value);
    serde_json::from_str(&text).expect("is JSON")
}

#[test]
fn ids_round_trip() {
    assert_eq!(
        round_trip(&ProjectId("mintswarm".into())),
        json!("mintswarm")
    );
    assert_eq!(round_trip(&AccountNickname("work".into())), json!("work"));
    assert_eq!(round_trip(&RunnerId("runner-1".into())), json!("runner-1"));
}

fn command(text: Option<&str>) -> CommandEvent {
    CommandEvent {
        issue: IssueNumber(226),
        pr: text.map(|_| PrNumber(420)),
        comment_id: 7,
        author: Login("maintainer".into()),
        observed_at: "2026-11-25T10:00:00Z".into(),
        text: text.map(Into::into),
    }
}

#[test]
fn every_command_has_a_variant_that_round_trips() {
    let commands: [(&str, fn(CommandEvent) -> WatchEvent); 9] = [
        ("approve", WatchEvent::Approve),
        ("changes", WatchEvent::Changes),
        ("reject", WatchEvent::Reject),
        ("veto", WatchEvent::Veto),
        ("steer", WatchEvent::Steer),
        ("continue", WatchEvent::Continue),
        ("accept", WatchEvent::Accept),
        ("replan", WatchEvent::Replan),
        ("cancel", WatchEvent::Cancel),
    ];
    for (word, variant) in commands {
        for text in [Some("use the other crate"), None] {
            let json = round_trip(&variant(command(text)));
            assert_eq!(json["kind"], json!(word));
        }
    }
}

#[test]
fn review_merged_and_issue_changes_round_trip() {
    let review = WatchEvent::Review(ReviewEvent {
        pr: PrNumber(420),
        review: Review {
            id: 1,
            author: Login("maintainer".into()),
            state: "APPROVED".into(),
            submitted_at: Some("2026-11-25T10:00:00Z".into()),
            commit: Some(RevId("a".repeat(40))),
            url: "https://example.invalid/review/1".into(),
        },
        observed_at: "2026-11-25T10:01:00Z".into(),
    });
    assert_eq!(round_trip(&review)["kind"], json!("review"));

    let merged = WatchEvent::Merged(MergedEvent {
        pr: PrNumber(420),
        head: RevId("b".repeat(40)),
        observed_at: "2026-11-25T10:02:00Z".into(),
    });
    assert_eq!(round_trip(&merged)["kind"], json!("merged"));

    let changes = [
        (IssueChange::Closed, "closed"),
        (IssueChange::Reopened, "reopened"),
        (
            IssueChange::Labelled {
                label: "mintswarm".into(),
            },
            "labelled",
        ),
        (
            IssueChange::Unlabelled {
                label: "mintswarm".into(),
            },
            "unlabelled",
        ),
        (IssueChange::Edited, "edited"),
    ];
    for (change, word) in changes {
        let event = WatchEvent::IssueChanged(IssueChangedEvent {
            issue: IssueNumber(226),
            change,
            observed_at: "2026-11-25T10:03:00Z".into(),
        });
        let json = round_trip(&event);
        assert_eq!(json["kind"], json!("issue_changed"));
        assert_eq!(json["change"]["change"], json!(word));
    }
}

#[test]
fn lease_epoch_and_directive_id_live_in_ids() {
    assert_eq!(round_trip(&LeaseEpoch(3)), json!(3));
    assert_eq!(round_trip(&DirectiveId("d-1".into())), json!("d-1"));
}
