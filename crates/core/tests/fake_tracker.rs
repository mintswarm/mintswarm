//! The in-memory tracker: seeded reads, recorded writes and the refusals it shares with the
//! GitHub tracker.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use mintswarm_core::testing::tracker::{FakeTracker, FakeTrackerError, FakeWrite};
use mintswarm_domain::field::{
    FieldError, FieldFinding, FieldRead, FieldRef, FieldSource, FieldValue,
};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{
    Artifact, Collaborators, Comment, CommentEdits, Comments, Cursor, Dependency, Direction,
    Fields, Issue, IssueType, Issues, Link, LinkKind, Links, Login, Reactions, RepoRole, Statuses,
    SubItem,
};
use mintswarm_github::NOTE_MARKER;

const BOT: &str = "mintswarm-dev[bot]";
const ISSUE: IssueNumber = IssueNumber(7);
const KEY: &str = "<!-- mintswarm: blockers open=12 -->";

fn issue(number: u64) -> Issue {
    Issue {
        number: IssueNumber(number),
        title: format!("Issue {number}"),
        body: String::new(),
        state: "open".into(),
        labels: vec!["mintswarm".into()],
        issue_type: Some(IssueType::Implementation),
        author: Login("maintainer".into()),
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        sub_issues: 0,
        blocked_by: 0,
    }
}

fn comment(id: u64, updated_at: &str) -> Comment {
    Comment {
        id,
        author: Login("maintainer".into()),
        body: format!("Comment {id}."),
        created_at: updated_at.into(),
        updated_at: updated_at.into(),
        url: String::new(),
    }
}

fn effort_source() -> FieldSource {
    FieldSource::Field {
        name: "Effort".into(),
    }
}

fn effort() -> FieldRef {
    FieldRef {
        source: effort_source(),
        options: ["S", "M", "L"].map(String::from).to_vec(),
    }
}

fn note(text: &str) -> String {
    format!("{NOTE_MARKER}\n{KEY}\n{text}")
}

fn fake() -> FakeTracker {
    let fake = FakeTracker::new(Login(BOT.into())).with_statuses(["Queued", "Done"]);
    fake.add_issue(issue(7));
    fake
}

/// Drives a tracker through every capability the GitHub tracker implements, so the fake is
/// shown at compile time to implement the same traits.
async fn drive<T>(tracker: &T) -> (FieldRead, Option<String>, Comment)
where
    T: Issues + Links + Fields + Statuses + Comments + Reactions + CommentEdits + Collaborators,
{
    tracker.read_issue(ISSUE, None).await.unwrap();
    tracker.read_parent(ISSUE).await.unwrap();
    tracker
        .read_permission(Login("maintainer".into()))
        .await
        .unwrap();
    tracker.read_links(ISSUE).await.unwrap();
    tracker
        .read_dependencies(ISSUE, Direction::BlockedBy)
        .await
        .unwrap();
    tracker
        .set_field(ISSUE, &effort(), Some("M"))
        .await
        .unwrap();
    tracker.set_status(ISSUE, "Done".into()).await.unwrap();
    let posted = tracker.post_comment(ISSUE, note("Hello.")).await.unwrap();
    (
        tracker.read_field(ISSUE, &effort()).await.unwrap(),
        tracker.read_status(ISSUE).await.unwrap(),
        posted,
    )
}

#[tokio::test]
async fn it_implements_the_capabilities_of_the_github_tracker() {
    let fake = fake();
    let (field, status, posted) = drive(&fake).await;
    assert_eq!(
        field,
        FieldRead::Value(FieldValue {
            field_id: None,
            option_id: None,
            name: "M".into(),
        })
    );
    assert_eq!(status.as_deref(), Some("Done"));
    assert_eq!(posted.author, Login(BOT.into()));
    assert_eq!(fake.comments(&ISSUE), [posted]);
}

#[tokio::test]
async fn seeded_reads_come_back() {
    let fake = fake();
    fake.add_comment(&ISSUE, comment(1, "2026-11-20T08:00:00Z"));
    fake.add_comment(&ISSUE, comment(2, "2026-11-22T08:00:00Z"));
    let links = vec![Link {
        kind: LinkKind::Parent,
        number: 3,
    }];
    fake.set_links(&ISSUE, links.clone());
    let blockers = vec![Dependency {
        repository: "acme/widgets".into(),
        number: IssueNumber(12),
        open: true,
    }];
    fake.set_dependencies(&ISSUE, Direction::BlockedBy, blockers.clone());
    fake.put_field(&ISSUE, effort_source(), "L");
    fake.put_status(&ISSUE, "Queued");

    let read = fake.read_issue(ISSUE, None).await.unwrap();
    assert_eq!(read.issue, issue(7));
    assert_eq!(read.comments.len(), 2);
    assert_eq!(read.cursor, Cursor("2026-11-22T08:00:00Z".into()));
    // `since` is inclusive, as on GitHub.
    let read = fake
        .read_issue(ISSUE, Some(Cursor("2026-11-22T08:00:00Z".into())))
        .await
        .unwrap();
    assert_eq!(read.comments, [comment(2, "2026-11-22T08:00:00Z")]);
    assert_eq!(fake.read_links(ISSUE).await.unwrap(), links);
    assert_eq!(
        fake.read_dependencies(ISSUE, Direction::BlockedBy)
            .await
            .unwrap(),
        blockers
    );
    assert!(
        fake.read_dependencies(ISSUE, Direction::Blocking)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fake.read_field(ISSUE, &effort()).await.unwrap().finding(),
        None
    );
    assert_eq!(
        fake.read_status(ISSUE).await.unwrap().as_deref(),
        Some("Queued")
    );
    assert!(fake.writes().is_empty());
}

#[tokio::test]
async fn accepted_writes_are_recorded_in_order() {
    let fake = fake();
    fake.set_now("2026-11-26T12:00:00Z");
    fake.set_field(ISSUE, &effort(), Some("M")).await.unwrap();
    fake.set_status(ISSUE, "Done".into()).await.unwrap();
    fake.set_status(ISSUE, "Done".into()).await.unwrap();
    let posted = fake.post_comment(ISSUE, note("One.")).await.unwrap();
    fake.set_field(ISSUE, &effort(), None).await.unwrap();
    assert_eq!(
        fake.writes(),
        [
            FakeWrite::SetField {
                issue: ISSUE,
                field: effort_source(),
                option: Some("M".into()),
            },
            FakeWrite::SetStatus {
                issue: ISSUE,
                status: "Done".into(),
            },
            FakeWrite::SetStatus {
                issue: ISSUE,
                status: "Done".into(),
            },
            FakeWrite::PostComment {
                issue: ISSUE,
                body: note("One."),
            },
            FakeWrite::SetField {
                issue: ISSUE,
                field: effort_source(),
                option: None,
            },
        ]
    );
    assert_eq!(posted.created_at, "2026-11-26T12:00:00Z");
    assert_eq!(fake.field(&ISSUE, &effort_source()), None);
    assert_eq!(fake.status(&ISSUE).as_deref(), Some("Done"));
}

#[tokio::test]
async fn refused_writes_are_not_recorded() {
    let fake = fake();
    assert_eq!(
        fake.set_field(ISSUE, &effort(), Some("XXL")).await,
        Err(FieldError::Field(FieldFinding::OptionUnmapped))
    );
    assert_eq!(
        fake.set_status(ISSUE, "Parked".into()).await,
        Err(FakeTrackerError::UnknownStatus {
            name: "Parked".into()
        })
    );
    assert_eq!(
        fake.post_comment(ISSUE, format!("{NOTE_MARKER}\nNo key."))
            .await,
        Err(FakeTrackerError::Unmarked)
    );
    assert_eq!(
        fake.post_comment(ISSUE, format!("{KEY}\nNo note marker."))
            .await,
        Err(FakeTrackerError::Unmarked)
    );
    let recreated = FieldError::Field(FieldFinding::FieldRecreated);
    fake.fail_fields(&ISSUE, recreated.clone());
    assert_eq!(
        fake.set_field(ISSUE, &effort(), Some("M")).await,
        Err(recreated.clone())
    );
    assert_eq!(fake.read_field(ISSUE, &effort()).await, Err(recreated));
    assert!(fake.writes().is_empty());
    assert!(fake.comments(&ISSUE).is_empty());
    assert_eq!(fake.status(&ISSUE), None);
}

#[tokio::test]
async fn a_stored_value_the_map_does_not_list_is_unmapped() {
    let fake = fake();
    fake.put_field(&ISSUE, effort_source(), "Huge");
    assert_eq!(
        fake.read_field(ISSUE, &effort()).await,
        Err(FieldError::Field(FieldFinding::OptionUnmapped))
    );
}

#[tokio::test]
async fn an_unreadable_issue_fails_its_reads() {
    let fake = fake();
    fake.unreadable(&ISSUE);
    let unreadable = FakeTrackerError::Unreadable { issue: ISSUE };
    assert_eq!(fake.read_issue(ISSUE, None).await, Err(unreadable.clone()));
    assert_eq!(fake.read_links(ISSUE).await, Err(unreadable.clone()));
    assert_eq!(
        fake.read_dependencies(ISSUE, Direction::Blocking).await,
        Err(unreadable.clone())
    );
    assert_eq!(fake.read_status(ISSUE).await, Err(unreadable));
    assert!(matches!(
        fake.read_field(ISSUE, &effort()).await,
        Err(FieldError::Transport(_))
    ));
}

#[tokio::test]
async fn an_unknown_issue_is_no_such_issue() {
    let fake = fake();
    assert_eq!(
        fake.read_issue(IssueNumber(99), None).await,
        Err(FakeTrackerError::NoSuchIssue {
            issue: IssueNumber(99)
        })
    );
}

#[tokio::test]
async fn unsupported_operations_are_refused() {
    let fake = fake();
    let item = SubItem {
        title: "A child".into(),
        body: String::new(),
        labels: Vec::new(),
        issue_type: None,
    };
    let errors = [
        fake.fetch_ready().await.map(|_| ()).unwrap_err(),
        fake.create_sub_item(ISSUE, item)
            .await
            .map(|_| ())
            .unwrap_err(),
        fake.set_issue_type(ISSUE, IssueType::Plan)
            .await
            .unwrap_err(),
        fake.link_dependency(ISSUE, IssueNumber(8))
            .await
            .unwrap_err(),
    ];
    assert_eq!(
        errors,
        [
            "fetch_ready",
            "create_sub_item",
            "set_issue_type",
            "link_dependency"
        ]
        .map(|operation| FakeTrackerError::Unsupported { operation })
    );
    assert!(fake.writes().is_empty());
}

fn artifact(body: &str) -> Artifact {
    Artifact {
        name: "Plan".into(),
        body: body.into(),
    }
}

/// The fake with comment 1 and artifact 1 seeded on the issue.
fn editable() -> FakeTracker {
    let fake = fake();
    fake.add_comment(&ISSUE, comment(1, "2026-11-20T08:00:00Z"));
    fake.add_artifact(&ISSUE, 1, artifact("Version 1."));
    fake
}

#[tokio::test]
async fn reactions_and_edits_are_recorded_in_order() {
    let fake = editable();
    fake.set_now("2026-11-26T12:00:00Z");
    fake.react_plus_one(1).await.unwrap();
    fake.react_plus_one(1).await.unwrap();
    let edited = fake.edit_comment(1, "Edited.".into()).await.unwrap();
    let updated = fake
        .update_artifact(ISSUE, 1, "Version 2.".into())
        .await
        .unwrap();
    assert_eq!(
        fake.writes(),
        [
            FakeWrite::React { comment: 1 },
            FakeWrite::React { comment: 1 },
            FakeWrite::EditComment {
                comment: 1,
                body: "Edited.".into(),
            },
            FakeWrite::UpdateArtifact {
                issue: ISSUE,
                number: 1,
                body: "Version 2.".into(),
            },
        ]
    );
    assert!(fake.reacted(1));
    assert_eq!(edited.body, "Edited.");
    assert_eq!(edited.created_at, "2026-11-20T08:00:00Z");
    assert_eq!(edited.updated_at, "2026-11-26T12:00:00Z");
    assert_eq!(fake.comments(&ISSUE), [edited]);
    assert_eq!(updated, artifact("Version 2."));
    assert_eq!(fake.artifact(&ISSUE, 1), Some(updated));
}

#[tokio::test]
async fn refused_reactions_and_edits_are_not_recorded() {
    let fake = editable();
    fake.refuse_comment(1);
    fake.refuse_artifact(&ISSUE, 1);
    assert_eq!(fake.react_plus_one(1).await, Err(FakeTrackerError::Refused));
    assert_eq!(
        fake.edit_comment(1, "Edited.".into()).await,
        Err(FakeTrackerError::Refused)
    );
    assert_eq!(
        fake.update_artifact(ISSUE, 1, "Version 2.".into()).await,
        Err(FakeTrackerError::Refused)
    );
    assert!(fake.writes().is_empty());
    assert!(!fake.reacted(1));
    assert_eq!(fake.comments(&ISSUE), [comment(1, "2026-11-20T08:00:00Z")]);
    assert_eq!(fake.artifact(&ISSUE, 1), Some(artifact("Version 1.")));
}

#[tokio::test]
async fn unknown_comments_and_artifacts_are_not_found() {
    let fake = editable();
    assert_eq!(
        fake.react_plus_one(99).await,
        Err(FakeTrackerError::NoSuchComment { comment: 99 })
    );
    assert_eq!(
        fake.edit_comment(99, "Edited.".into()).await,
        Err(FakeTrackerError::NoSuchComment { comment: 99 })
    );
    assert_eq!(
        fake.update_artifact(ISSUE, 2, "Version 2.".into()).await,
        Err(FakeTrackerError::NoSuchArtifact {
            issue: ISSUE,
            number: 2
        })
    );
    assert!(fake.writes().is_empty());
    assert!(!fake.reacted(99));
}

#[tokio::test]
async fn permissions_are_the_seeded_roles() {
    let fake = fake();
    fake.set_role(Login("maintainer".into()), RepoRole::Admin);
    assert_eq!(
        fake.read_permission(Login("maintainer".into())).await,
        Ok(RepoRole::Admin)
    );
    assert_eq!(
        fake.read_permission(Login("stranger".into())).await,
        Ok(RepoRole::NoAccess)
    );
}

#[tokio::test]
async fn the_parent_agrees_with_the_links() {
    let fake = fake();
    assert_eq!(fake.read_parent(ISSUE).await, Ok(None));
    fake.set_links(
        &ISSUE,
        vec![
            Link {
                kind: LinkKind::SubItem,
                number: 9,
            },
            Link {
                kind: LinkKind::Parent,
                number: 3,
            },
        ],
    );
    let parent = fake
        .read_links(ISSUE)
        .await
        .unwrap()
        .into_iter()
        .find(|link| link.kind == LinkKind::Parent)
        .map(|link| IssueNumber(link.number));
    assert_eq!(parent, Some(IssueNumber(3)));
    assert_eq!(fake.read_parent(ISSUE).await, Ok(parent));
}
