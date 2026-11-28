//! The in-memory tracker: seeded reads, recorded writes and the refusals it shares with the
//! GitHub tracker.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use mintswarm_core::testing::tracker::{FakeMerge, FakeTracker, FakeTrackerError, FakeWrite};
use mintswarm_domain::field::{
    FieldError, FieldFinding, FieldRead, FieldRef, FieldSource, FieldValue,
};
use mintswarm_domain::hosting::CodeOwnersFile;
use mintswarm_domain::ids::{IssueNumber, PrNumber, RevId};
use mintswarm_domain::tracker::{
    Artifact, ArtifactReads, AssignmentReason, Assignments, ChangedFile, CheckRun, CheckState,
    CheckSuite, CodeOwners, Collaborators, CombinedStatus, Comment, CommentEdits, Comments, Cursor,
    Dependency, Direction, Fields, Issue, IssueType, Issues, Link, LinkKind, Links, Login,
    MergeOutcome, Merged, Merges, ParentIssue, PullDetail, Reactions, RepoRole, RequestedReviewers,
    Review, ReviewComment, ReviewThread, Reviews, Statuses, SubItem,
};
use mintswarm_domain::vcs::{BranchName, RepoPath};
use mintswarm_github::{AssignedEvent, NOTE_MARKER, Token};
use mintswarm_rules::render::decision::refusal;

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
        author_is_bot: false,
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        sub_issues: 0,
        blocked_by: 0,
    }
}

fn comment(id: u64, updated_at: &str) -> Comment {
    Comment {
        id,
        author: Login("maintainer".into()),
        author_is_bot: false,
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
    T: Issues
        + Links
        + Fields
        + Statuses
        + Comments
        + Assignments
        + Reactions
        + CommentEdits
        + ArtifactReads
        + Collaborators
        + Reviews
        + CodeOwners
        + Merges<MergeToken = Token>,
{
    tracker.read_check_runs(RevId(HEAD.into())).await.unwrap();
    tracker.read_codeowners(RevId(HEAD.into())).await.unwrap();
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
    // A parent link to an issue that is not seeded is the fake's not-found error.
    assert_eq!(
        fake.read_parent(ISSUE).await,
        Err(FakeTrackerError::NoSuchIssue {
            issue: IssueNumber(3)
        })
    );
    fake.add_issue(Issue {
        author: Login("helper".into()),
        author_is_bot: true,
        ..issue(3)
    });
    let parent = fake
        .read_links(ISSUE)
        .await
        .unwrap()
        .into_iter()
        .find(|link| link.kind == LinkKind::Parent)
        .map(|link| IssueNumber(link.number));
    assert_eq!(parent, Some(IssueNumber(3)));
    assert_eq!(
        fake.read_parent(ISSUE).await,
        Ok(Some(ParentIssue {
            number: IssueNumber(3),
            author: Some(Login("helper".into())),
            author_is_bot: true,
        }))
    );
}

const PR: PrNumber = PrNumber(400);
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const MOVED: &str = "89abcdef0123456789abcdef0123456789abcdef";
const MERGE_TOKEN: &str = "ghs_f4keM3rgeT0ken";

fn pull() -> PullDetail {
    PullDetail {
        number: PR,
        url: "https://github.com/acme/widgets/pull/400".into(),
        title: "feat(domain): GitHub REST reads".into(),
        state: "open".into(),
        draft: false,
        base: BranchName("master".into()),
        branch: BranchName("mintswarm/180-rest-reads".into()),
        head: RevId(HEAD.into()),
        head_repo: Some("acme/widgets".into()),
        labels: vec!["mintswarm".into()],
        mergeable: Some(true),
        mergeable_state: Some("clean".into()),
        merged: false,
        merge_commit: None,
        changed_files: 1,
    }
}

fn with_pull() -> FakeTracker {
    let fake = fake();
    fake.add_pull(pull());
    fake
}

fn merge_token() -> Token {
    Token::new(MERGE_TOKEN.to_owned(), u64::MAX)
}

#[tokio::test]
async fn seeded_pr_reads_come_back() {
    let fake = with_pull();
    let head = RevId(HEAD.into());
    let reviews = vec![Review {
        id: 77,
        author: Login("maintainer".into()),
        state: "APPROVED".into(),
        submitted_at: Some("2026-11-25T10:00:00Z".into()),
        commit: Some(head.clone()),
        url: String::new(),
        author_is_bot: false,
    }];
    let comments = vec![ReviewComment {
        id: 901,
        review: Some(77),
        author: Login("maintainer".into()),
        author_is_bot: false,
        body: "Nit.".into(),
        created_at: "2026-11-25T10:05:00Z".into(),
        updated_at: "2026-11-25T10:05:00Z".into(),
        url: String::new(),
    }];
    let threads = vec![ReviewThread {
        id: "PRRT_1".into(),
        resolved: false,
    }];
    let requested = RequestedReviewers {
        users: vec![Login("maintainer".into())],
        teams: vec!["reviewers".into()],
    };
    let runs = vec![CheckRun {
        id: 3001,
        name: "rust-test".into(),
        status: "completed".into(),
        conclusion: Some("success".into()),
        app: Some("github-actions".into()),
        suite: Some(2001),
    }];
    let suites = vec![CheckSuite {
        id: 2001,
        app: Some("github-actions".into()),
        status: Some("completed".into()),
        conclusion: Some("success".into()),
        latest_check_runs_count: 1,
    }];
    let status = CombinedStatus {
        state: "success".into(),
        total_count: 1,
        statuses: vec![CheckState {
            name: "mintswarm/gates".into(),
            conclusion: Some("success".into()),
            source: "status".into(),
            app: None,
        }],
    };
    let files = vec![ChangedFile {
        path: "crates/github/src/pulls.rs".into(),
        previous_path: None,
        status: "modified".into(),
    }];
    fake.set_reviews(&PR, reviews.clone());
    fake.set_review_comments(&PR, comments.clone());
    fake.set_review_threads(&PR, threads.clone());
    fake.set_requested_reviewers(&PR, requested.clone());
    fake.set_check_runs(&head, runs.clone());
    fake.set_check_suites(&head, suites.clone());
    fake.set_statuses(&head, status.clone());
    fake.set_changed_files(&PR, files.clone());
    assert_eq!(fake.read_pull(PR).await, Ok(pull()));
    assert_eq!(fake.read_reviews(PR).await, Ok(reviews));
    assert_eq!(fake.read_review_comments(PR).await, Ok(comments));
    assert_eq!(fake.read_review_threads(PR).await, Ok(threads));
    assert_eq!(fake.read_requested_reviewers(PR).await, Ok(requested));
    assert_eq!(fake.read_check_runs(head.clone()).await, Ok(runs));
    assert_eq!(fake.read_check_suites(head.clone()).await, Ok(suites));
    assert_eq!(fake.read_statuses(head).await, Ok(status));
    assert_eq!(fake.read_changed_files(PR).await, Ok(files));
    assert!(fake.writes().is_empty());
}

#[tokio::test]
async fn unseeded_pr_reads_are_empty_or_no_such_pull() {
    let fake = with_pull();
    let other = RevId(MOVED.into());
    assert_eq!(fake.read_reviews(PR).await, Ok(Vec::new()));
    assert_eq!(
        fake.read_requested_reviewers(PR).await,
        Ok(RequestedReviewers::default())
    );
    assert_eq!(fake.read_check_runs(other.clone()).await, Ok(Vec::new()));
    let status = fake.read_statuses(other).await.unwrap();
    assert_eq!(
        (
            status.state.as_str(),
            status.total_count,
            status.statuses.len()
        ),
        ("pending", 0, 0)
    );
    let unknown = PrNumber(401);
    let no_such = Err(FakeTrackerError::NoSuchPull {
        pr: unknown.clone(),
    });
    assert_eq!(fake.read_pull(unknown.clone()).await.map(|_| ()), no_such);
    assert_eq!(
        fake.read_reviews(unknown.clone()).await.map(|_| ()),
        no_such
    );
    assert_eq!(fake.read_changed_files(unknown).await.map(|_| ()), no_such);
}

#[tokio::test]
async fn codeowners_is_the_first_seeded_of_three_paths_or_none() {
    let fake = fake();
    let head = RevId(HEAD.into());
    assert_eq!(fake.read_codeowners(head.clone()).await, Ok(None));
    fake.put_file(&head, "docs/CODEOWNERS", "* @acme/docs\n");
    fake.put_file(&head, "CODEOWNERS", "* @acme/maintainers\n");
    // Another commit's file and a path outside the three are not read.
    fake.put_file(
        &RevId(MOVED.into()),
        ".github/CODEOWNERS",
        "* @acme/other\n",
    );
    fake.put_file(&head, "src/CODEOWNERS", "* @acme/src\n");
    assert_eq!(
        fake.read_codeowners(head.clone()).await,
        Ok(Some(CodeOwnersFile {
            path: RepoPath(b"CODEOWNERS".to_vec()),
            text: "* @acme/maintainers\n".into(),
        }))
    );
    fake.put_file(&head, ".github/CODEOWNERS", "* @acme/github\n");
    assert_eq!(
        fake.read_codeowners(head)
            .await
            .unwrap()
            .map(|file| file.path),
        Some(RepoPath(b".github/CODEOWNERS".to_vec()))
    );
}

#[tokio::test]
async fn codeowners_at_a_moving_ref_is_refused() {
    let fake = fake();
    for rev in ["master", "0123456", &HEAD.to_uppercase()] {
        fake.put_file(&RevId(rev.into()), "CODEOWNERS", "* @acme/maintainers\n");
        assert_eq!(
            fake.read_codeowners(RevId(rev.into())).await,
            Err(FakeTrackerError::InvalidRev {
                rev: RevId(rev.into())
            })
        );
    }
}

#[tokio::test]
async fn a_merge_at_the_head_succeeds_and_is_recorded_with_its_token() {
    let fake = with_pull();
    let outcome = fake
        .merge(
            &merge_token(),
            PR,
            RevId(HEAD.into()),
            "Title (#400)".into(),
        )
        .await;
    let commit = RevId(format!("{:040x}", 400));
    assert_eq!(
        outcome,
        Ok(MergeOutcome::Merged(Merged {
            sha: commit.clone()
        }))
    );
    let merged = fake.pull(&PR).unwrap();
    assert!(merged.merged);
    assert_eq!(merged.state, "closed");
    assert_eq!(merged.merge_commit, Some(commit));
    let writes = fake.writes();
    assert_eq!(writes.len(), 1);
    match &writes[0] {
        FakeWrite::Merge {
            pr,
            sha,
            title,
            token,
        } => {
            assert_eq!(
                (pr, sha, title.as_str()),
                (&PR, &RevId(HEAD.into()), "Title (#400)")
            );
            assert!(
                token.expose() == MERGE_TOKEN,
                "the merge is not recorded with the token passed in"
            );
        }
        other => panic!("not a merge: {other:?}"),
    }
    // A second merge of the merged pull request is refused.
    assert_eq!(
        fake.merge(&merge_token(), PR, RevId(HEAD.into()), "Title".into())
            .await,
        Ok(MergeOutcome::NotMergeable)
    );
}

#[tokio::test]
async fn a_head_moved_between_check_and_merge_is_head_moved() {
    let fake = with_pull();
    let checked = fake.read_pull(PR).await.unwrap().head;
    fake.move_head(&PR, RevId(MOVED.into()));
    assert_eq!(
        fake.merge(&merge_token(), PR, checked, "Title".into())
            .await,
        Ok(MergeOutcome::HeadMoved)
    );
    let pull = fake.pull(&PR).unwrap();
    assert!(!pull.merged);
    assert_eq!(pull.head, RevId(MOVED.into()));
}

#[tokio::test]
async fn a_scripted_refusal_is_not_mergeable() {
    let fake = with_pull();
    fake.script_merge(&PR, FakeMerge::NotMergeable);
    let head = RevId(HEAD.into());
    assert_eq!(
        fake.merge(&merge_token(), PR, head.clone(), "Title".into())
            .await,
        Ok(MergeOutcome::NotMergeable)
    );
    assert!(!fake.pull(&PR).unwrap().merged);
    // The script is used up: the next merge is answered from the state.
    assert!(matches!(
        fake.merge(&merge_token(), PR, head, "Title".into()).await,
        Ok(MergeOutcome::Merged(_))
    ));
}

#[tokio::test]
async fn a_lost_merge_says_whether_it_may_have_landed() {
    let fake = with_pull();
    fake.script_merge(&PR, FakeMerge::Lost { landed: true });
    fake.script_merge(&PR, FakeMerge::Lost { landed: false });
    let mut landed = Vec::new();
    for _ in 0..2 {
        let error = fake
            .merge(&merge_token(), PR, RevId(HEAD.into()), "Title".into())
            .await
            .unwrap_err();
        landed.push(FakeTracker::may_have_landed(&error));
    }
    assert_eq!(landed, [true, false]);
    assert!(!FakeTracker::may_have_landed(
        &FakeTrackerError::NoSuchPull { pr: PR }
    ));
}

#[tokio::test]
async fn scripted_merges_carry_their_effect_on_the_pull_request() {
    let fake = with_pull();
    let head = RevId(HEAD.into());
    let moved = RevId(MOVED.into());
    let commit = RevId("c".repeat(40));
    // A scripted head moved moves the head.
    fake.script_merge(
        &PR,
        FakeMerge::HeadMoved {
            head: moved.clone(),
        },
    );
    assert_eq!(
        fake.merge(&merge_token(), PR, head.clone(), "Title".into())
            .await,
        Ok(MergeOutcome::HeadMoved)
    );
    let pull = fake.read_pull(PR).await.unwrap();
    assert_eq!((pull.head, pull.merged), (moved.clone(), false));
    // A scripted refusal leaves the pull request as it is.
    fake.script_merge(&PR, FakeMerge::NotMergeable);
    assert_eq!(
        fake.merge(&merge_token(), PR, moved.clone(), "Title".into())
            .await,
        Ok(MergeOutcome::NotMergeable)
    );
    let pull = fake.read_pull(PR).await.unwrap();
    assert_eq!(
        (pull.head, pull.merged, pull.state.as_str()),
        (moved.clone(), false, "open")
    );
    // A lost merge that did not land leaves it as it is too.
    fake.script_merge(&PR, FakeMerge::Lost { landed: true });
    let error = fake
        .merge(&merge_token(), PR, moved.clone(), "Title".into())
        .await
        .unwrap_err();
    assert!(FakeTracker::may_have_landed(&error));
    let pull = fake.read_pull(PR).await.unwrap();
    assert_eq!((pull.merged, pull.merge_commit), (false, None));
    // A lost merge that landed after all is merged on read-back.
    fake.script_merge(
        &PR,
        FakeMerge::LostButMerged(Merged {
            sha: commit.clone(),
        }),
    );
    let error = fake
        .merge(&merge_token(), PR, moved.clone(), "Title".into())
        .await
        .unwrap_err();
    assert!(FakeTracker::may_have_landed(&error));
    let pull = fake.read_pull(PR).await.unwrap();
    assert_eq!(
        (pull.merged, pull.state.as_str(), pull.merge_commit),
        (true, "closed", Some(commit.clone()))
    );
    // A scripted merge is merged on read-back, with its commit.
    let fake = with_pull();
    fake.script_merge(
        &PR,
        FakeMerge::Merged(Merged {
            sha: commit.clone(),
        }),
    );
    assert_eq!(
        fake.merge(&merge_token(), PR, head, "Title".into()).await,
        Ok(MergeOutcome::Merged(Merged {
            sha: commit.clone()
        }))
    );
    let pull = fake.read_pull(PR).await.unwrap();
    assert_eq!(
        (pull.merged, pull.state.as_str(), pull.merge_commit),
        (true, "closed", Some(commit))
    );
}

#[tokio::test]
async fn a_scripted_merge_of_an_unknown_pull_request_is_no_such_pull() {
    let fake = with_pull();
    let other = PrNumber(401);
    fake.script_merge(&other, FakeMerge::NotMergeable);
    assert_eq!(
        fake.merge(
            &merge_token(),
            other.clone(),
            RevId(HEAD.into()),
            "Title".into()
        )
        .await,
        Err(FakeTrackerError::NoSuchPull { pr: other })
    );
}

#[tokio::test]
async fn an_unmergeable_pull_request_is_not_mergeable_before_its_head_is_checked() {
    let head = RevId(HEAD.into());
    let moved = RevId(MOVED.into());
    let unmergeable = [
        PullDetail {
            merged: true,
            state: "closed".into(),
            merge_commit: Some(RevId("c".repeat(40))),
            ..pull()
        },
        PullDetail {
            state: "closed".into(),
            ..pull()
        },
        PullDetail {
            mergeable: Some(false),
            ..pull()
        },
    ];
    for seeded in unmergeable {
        for sha in [&head, &moved] {
            let fake = fake();
            fake.add_pull(seeded.clone());
            assert_eq!(
                fake.merge(&merge_token(), PR, sha.clone(), "Title".into())
                    .await,
                Ok(MergeOutcome::NotMergeable),
                "{seeded:?} at {sha:?}"
            );
            assert_eq!(fake.pull(&PR), Some(seeded.clone()));
        }
    }
    // Mergeability still being worked out does not refuse it.
    let fake = fake();
    fake.add_pull(PullDetail {
        mergeable: None,
        ..pull()
    });
    assert!(matches!(
        fake.merge(&merge_token(), PR, head, "Title".into()).await,
        Ok(MergeOutcome::Merged(_))
    ));
}

#[tokio::test]
async fn every_merge_call_is_recorded_whatever_its_outcome() {
    let fake = with_pull();
    fake.script_merge(&PR, FakeMerge::Lost { landed: true });
    let head = RevId(HEAD.into());
    let moved = RevId(MOVED.into());
    let _ = fake
        .merge(&merge_token(), PR, head.clone(), "Lost".into())
        .await;
    let _ = fake
        .merge(&merge_token(), PR, moved.clone(), "Moved".into())
        .await;
    let _ = fake
        .merge(
            &merge_token(),
            PrNumber(401),
            head.clone(),
            "Unknown".into(),
        )
        .await;
    let _ = fake
        .merge(&merge_token(), PR, head.clone(), "Merged".into())
        .await;
    let recorded: Vec<_> = fake
        .writes()
        .into_iter()
        .map(|write| match write {
            FakeWrite::Merge {
                pr,
                sha,
                title,
                token,
            } => {
                assert!(
                    token.expose() == MERGE_TOKEN,
                    "{title}: not the token passed in"
                );
                assert!(
                    !format!("{token:?}").contains(MERGE_TOKEN),
                    "the recorded token's Debug shows its value"
                );
                (pr, sha, title)
            }
            other => panic!("not a merge: {other:?}"),
        })
        .collect();
    assert_eq!(
        recorded,
        [
            (PR, head.clone(), "Lost".to_owned()),
            (PR, moved, "Moved".to_owned()),
            (PrNumber(401), head.clone(), "Unknown".to_owned()),
            (PR, head, "Merged".to_owned()),
        ]
    );
}

#[tokio::test]
async fn a_keyed_body_without_the_note_marker_posts() {
    let fake = fake();
    let body = format!("{KEY}\nNo note marker.");
    let posted = fake.post_comment(ISSUE, body.clone()).await.unwrap();
    assert_eq!(posted.body, body);
    assert_eq!(
        fake.writes(),
        [FakeWrite::PostComment { issue: ISSUE, body }]
    );
}

#[tokio::test]
async fn an_answer_is_posted_keyed_by_its_answer_line() {
    let fake = fake();
    let body = refusal(8800, "no package awaits approval");
    assert!(
        body.starts_with("<!-- mintswarm: answer 8800 -->\n"),
        "{body}"
    );
    let posted = fake.post_comment(ISSUE, body.clone()).await.unwrap();
    assert_eq!(posted.author, Login(BOT.into()));
    assert_eq!(fake.comments(&ISSUE), [posted]);
    assert_eq!(
        fake.writes(),
        [FakeWrite::PostComment { issue: ISSUE, body }]
    );
}

/// The plane's own logins in the release tests: an App's bot login, and a login without
/// `[bot]`.
const OWN: [&str; 2] = ["widgets-plane[bot]", "plane-operator"];

fn assigned(assignee: &str, actor: &str, app: Option<&str>, id: u64) -> AssignedEvent {
    AssignedEvent {
        assignee: Login(assignee.into()),
        actor: Some(Login(actor.into())),
        assigner: Some(Login(actor.into())),
        app: app.map(str::to_owned),
        created_at: Some("2026-11-26T12:00:00Z".into()),
        id: Some(id),
    }
}

/// A fake with own logins, `alice` assigned by an own login, `erin` through the App of the own
/// `[bot]` login, `bob` by a person and `carol` with no event.
fn assigned_fake(own: &[&str]) -> FakeTracker {
    let fake = FakeTracker::new(Login(BOT.into()))
        .with_own_logins(own.iter().map(|login| Login((*login).into())).collect());
    fake.add_issue(issue(7));
    fake.seed_assignment(&ISSUE, assigned("alice", "plane-operator", None, 1));
    fake.seed_assignment(&ISSUE, assigned("bob", "maintainer", None, 2));
    fake.seed_assignment(
        &ISSUE,
        assigned("erin", "someone-else", Some("widgets-plane"), 3),
    );
    fake.seed_assignee(&ISSUE, Login("carol".into()));
    fake
}

fn logins(names: &[&str]) -> Vec<Login> {
    names.iter().map(|name| Login((*name).into())).collect()
}

#[tokio::test]
async fn release_removes_the_own_assignments_and_keeps_a_person_s() {
    let fake = assigned_fake(&OWN);
    fake.release(ISSUE).await.unwrap();
    assert_eq!(fake.assignees(&ISSUE), logins(&["bob", "carol"]));
    assert_eq!(
        fake.writes(),
        [FakeWrite::Release {
            issue: ISSUE,
            logins: logins(&["alice", "erin"]),
        }]
    );
}

#[tokio::test]
async fn release_without_own_logins_removes_none() {
    let fake = assigned_fake(&[]);
    fake.release(ISSUE).await.unwrap();
    assert_eq!(
        fake.assignees(&ISSUE),
        logins(&["alice", "bob", "erin", "carol"])
    );
    assert!(fake.writes().is_empty());
}

#[tokio::test]
async fn assign_adds_new_logins_as_the_bot() {
    let fake = assigned_fake(&[BOT]);
    fake.assign(ISSUE, logins(&["Bob", "heidi"]), AssignmentReason::Plan)
        .await
        .unwrap();
    assert_eq!(
        fake.assignees(&ISSUE),
        logins(&["alice", "bob", "erin", "carol", "heidi"])
    );
    assert_eq!(
        fake.writes(),
        [FakeWrite::Assign {
            issue: ISSUE,
            logins: logins(&["heidi"]),
            reason: AssignmentReason::Plan,
        }]
    );
    // The bot made heidi's assignment, so a fake trusting the bot releases it.
    fake.release(ISSUE).await.unwrap();
    assert_eq!(
        fake.assignees(&ISSUE),
        logins(&["alice", "bob", "erin", "carol"])
    );
}

#[tokio::test]
async fn reads_a_seeded_artifact() {
    let fake = fake();
    fake.add_artifact(&ISSUE, 2, artifact("Decision."));
    assert_eq!(
        fake.read_artifact(ISSUE, 2).await,
        Ok(artifact("Decision."))
    );
    assert_eq!(
        fake.read_artifact(ISSUE, 3).await,
        Err(FakeTrackerError::NoSuchArtifact {
            issue: ISSUE,
            number: 3
        })
    );
}
