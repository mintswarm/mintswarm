//! The steward's merge-ready check against `land.py`'s refusals and the approval rule of the
//! 2026-11-27 answers: one passing world, changed one thing per case. Cases check reason
//! classes only, never wording.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use std::collections::BTreeMap;

use mintswarm_config::{AWAITING_MERGE, ActionTrust, People, Trust, TrustMatch};
use mintswarm_core::steward::checks::{
    ApproveHint, Facts, LinkedIssue, Outcome, Reason, evaluate, linked_issue,
};
use mintswarm_domain::approval::{ApprovalGroup, Eligibility, GroupKey};
use mintswarm_domain::codeowners::Owner;
use mintswarm_domain::ids::{IssueNumber, PrNumber, RevId};
use mintswarm_domain::tracker::{
    CheckRun, CheckState, CheckSuite, CombinedStatus, Comment, Issue, Login, PullDetail, RepoRole,
    RequestedReviewers, Review, ReviewThread,
};
use mintswarm_domain::vcs::BranchName;

const REPO: &str = "mintswarm/mintswarm";
const OWNER: &str = "zmij";
const BOT: &str = "mintswarm-dev[bot]";
const PR_URL: &str = "https://github.com/mintswarm/mintswarm/pull/568";
/// The approval's time; later facts come after it.
const APPROVED_AT: &str = "2026-11-27T10:00:00Z";
const LATER: &str = "2026-11-27T11:00:00Z";
const EARLIER: &str = "2026-11-27T09:00:00Z";
const STEER_MARKER: &str = "<!-- mintswarm: steer -->";
const APPROVAL_MARKER: &str = "<!-- mintswarm: steer approval -->";

fn sha(c: char) -> RevId {
    RevId(std::iter::repeat_n(c, 40).collect())
}

fn head() -> RevId {
    sha('a')
}

fn old() -> RevId {
    sha('b')
}

/// Everything the check reads, owned.
#[derive(Clone, Debug)]
struct World {
    repo: String,
    base: BranchName,
    pull: PullDetail,
    issue: Option<Issue>,
    state: Option<String>,
    ingest: Trust,
    approve_pr: ActionTrust,
    reviews: Vec<Review>,
    comments: Vec<Comment>,
    threads: Vec<ReviewThread>,
    requested: RequestedReviewers,
    checks_at: RevId,
    runs: Vec<CheckRun>,
    suites: Vec<CheckSuite>,
    statuses: CombinedStatus,
    groups: Vec<ApprovalGroup>,
    roles: BTreeMap<Login, RepoRole>,
    same_shape: Vec<RevId>,
}

fn review(id: u64, author: &str, state: &str, at: &str, commit: RevId) -> Review {
    Review {
        id,
        author: Login(author.to_owned()),
        state: state.to_owned(),
        submitted_at: Some(at.to_owned()),
        commit: Some(commit),
        url: format!("{PR_URL}#pullrequestreview-{id}"),
        author_is_bot: author.ends_with("[bot]"),
    }
}

fn comment(id: u64, author: &str, body: &str, created: &str, updated: &str) -> Comment {
    Comment {
        id,
        author: Login(author.to_owned()),
        body: body.to_owned(),
        created_at: created.to_owned(),
        updated_at: updated.to_owned(),
        url: format!("{PR_URL}#issuecomment-{id}"),
    }
}

fn run(id: u64, name: &str, suite: Option<u64>) -> CheckRun {
    CheckRun {
        id,
        name: name.to_owned(),
        status: "completed".to_owned(),
        conclusion: Some("success".to_owned()),
        app: Some("github-actions".to_owned()),
        suite,
    }
}

fn suite(id: u64, app: &str, runs: u64) -> CheckSuite {
    CheckSuite {
        id,
        app: Some(app.to_owned()),
        status: Some("completed".to_owned()),
        conclusion: Some("success".to_owned()),
        latest_check_runs_count: runs,
    }
}

fn status(name: &str, conclusion: Option<&str>) -> CheckState {
    CheckState {
        name: name.to_owned(),
        conclusion: conclusion.map(str::to_owned),
        source: "status".to_owned(),
        app: None,
    }
}

fn group(line: usize, pattern: &str, owner: &str) -> ApprovalGroup {
    ApprovalGroup {
        key: GroupKey::Rule {
            line,
            pattern: pattern.to_owned(),
        },
        eligible: Eligibility::Owners(vec![Owner::User(Login(owner.to_owned()))]),
        paths: vec![format!("{pattern}file.rs")],
    }
}

/// A pull request that passes every guard, approved at the head by its one code owner.
fn passing() -> World {
    World {
        repo: REPO.to_owned(),
        base: BranchName("master".to_owned()),
        pull: PullDetail {
            number: PrNumber(700),
            url: PR_URL.to_owned(),
            title: "feat(core): steward checks".to_owned(),
            state: "open".to_owned(),
            draft: false,
            base: BranchName("master".to_owned()),
            branch: BranchName("mintswarm/568-steward-approvals".to_owned()),
            head: head(),
            head_repo: Some(REPO.to_owned()),
            labels: vec!["mintswarm".to_owned()],
            mergeable: Some(true),
            mergeable_state: Some("clean".to_owned()),
            merged: false,
            merge_commit: None,
            changed_files: 5,
        },
        issue: Some(Issue {
            number: IssueNumber(568),
            title: "feat(core): steward checks".to_owned(),
            body: String::new(),
            state: "open".to_owned(),
            labels: vec!["mintswarm".to_owned()],
            issue_type: None,
            author: Login(OWNER.to_owned()),
            author_is_bot: false,
            url: "https://github.com/mintswarm/mintswarm/issues/568".to_owned(),
            sub_issues: 0,
            blocked_by: 0,
        }),
        state: Some(AWAITING_MERGE.to_owned()),
        ingest: Trust {
            labels: vec!["mintswarm".to_owned()],
            authors: Vec::new(),
            r#match: TrustMatch::Any,
        },
        approve_pr: ActionTrust {
            people: People::Codeowners,
            apps: Vec::new(),
        },
        reviews: vec![review(1, OWNER, "APPROVED", APPROVED_AT, head())],
        comments: Vec::new(),
        threads: vec![ReviewThread {
            id: "T1".to_owned(),
            resolved: true,
        }],
        requested: RequestedReviewers::default(),
        checks_at: head(),
        runs: vec![
            run(1, "gates-matrix", Some(7)),
            run(2, "gates (ubuntu)", Some(7)),
            run(3, "gates (macos)", Some(7)),
        ],
        suites: vec![suite(7, "github-actions", 3)],
        statuses: CombinedStatus {
            state: "success".to_owned(),
            total_count: 1,
            statuses: vec![status("ci/legacy", Some("success"))],
        },
        groups: vec![group(1, "", OWNER)],
        roles: BTreeMap::from([(Login(OWNER.to_owned()), RepoRole::Admin)]),
        same_shape: Vec::new(),
    }
}

fn facts(world: &World) -> Facts<'_> {
    Facts {
        repo: &world.repo,
        base: &world.base,
        pull: &world.pull,
        linked: world.issue.as_ref().map(|issue| LinkedIssue {
            issue,
            state: world.state.as_deref(),
        }),
        ingest: &world.ingest,
        approve_pr: &world.approve_pr,
        reviews: &world.reviews,
        comments: &world.comments,
        threads: &world.threads,
        requested: &world.requested,
        checks_at: &world.checks_at,
        runs: &world.runs,
        suites: &world.suites,
        statuses: &world.statuses,
        groups: &world.groups,
        roles: &world.roles,
        same_shape: &world.same_shape,
    }
}

fn outcome(change: impl FnOnce(&mut World)) -> Outcome {
    let mut world = passing();
    change(&mut world);
    evaluate(&facts(&world))
}

fn assert_ready(outcome: &Outcome) {
    assert!(outcome.merge_ready, "reasons: {:?}", outcome.reasons);
    assert!(outcome.reasons.is_empty());
    assert!(outcome.cover.covered);
}

fn assert_refused(outcome: &Outcome, expected: &[Reason]) {
    assert!(!outcome.merge_ready);
    for reason in expected {
        assert!(
            outcome.reasons.contains(reason),
            "{reason:?} missing from {:?}",
            outcome.reasons
        );
    }
}

fn run_mut(world: &mut World, id: u64) -> &mut CheckRun {
    world
        .runs
        .iter_mut()
        .find(|run| run.id == id)
        .expect("the run exists")
}

/// The old relayed approval steer, as `land.py` read it.
fn old_steer(sha: &RevId) -> String {
    format!(
        "{STEER_MARKER}\n{APPROVAL_MARKER}\nApproved head: {}",
        sha.0
    )
}

type Change = fn(&mut World);

#[test]
fn land_py_refusals() {
    let cases: &[(&str, Change, &[Reason])] = &[
        (
            "the head moved since the approval",
            |w| {
                w.pull.head = sha('c');
                w.checks_at = sha('c');
            },
            &[Reason::NotCovered, Reason::ShapeChanged],
        ),
        (
            "a failing check run",
            |w| run_mut(w, 2).conclusion = Some("failure".to_owned()),
            &[Reason::RunNotSucceeded],
        ),
        (
            "a pending check run",
            |w| {
                let run = run_mut(w, 3);
                run.status = "in_progress".to_owned();
                run.conclusion = None;
            },
            &[Reason::RunNotSucceeded],
        ),
        (
            "a skipped check run",
            |w| run_mut(w, 3).conclusion = Some("skipped".to_owned()),
            &[Reason::RunNotSucceeded],
        ),
        (
            "a neutral check run",
            |w| run_mut(w, 3).conclusion = Some("neutral".to_owned()),
            &[Reason::RunNotSucceeded],
        ),
        (
            "a failing status",
            |w| w.statuses.statuses[0].conclusion = Some("failure".to_owned()),
            &[Reason::StatusNotSucceeded],
        ),
        (
            "a pending status",
            |w| w.statuses.statuses[0].conclusion = None,
            &[Reason::StatusNotSucceeded],
        ),
        (
            "more statuses than one page holds",
            |w| w.statuses.total_count = 101,
            &[Reason::TooManyStatuses],
        ),
        (
            "more statuses than the list holds",
            |w| w.statuses.total_count = 2,
            &[Reason::TooManyStatuses],
        ),
        (
            "no gates-matrix check run",
            |w| w.runs.retain(|run| run.name != "gates-matrix"),
            &[Reason::NoGatesMatrix],
        ),
        (
            "a gates-matrix run created by another App",
            |w| run_mut(w, 1).app = Some("impostor".to_owned()),
            &[Reason::NoGatesMatrix],
        ),
        (
            "only gates-matrix has run, its suite still running",
            |w| {
                w.runs.retain(|run| run.id == 1);
                w.suites[0].status = Some("in_progress".to_owned());
                w.suites[0].conclusion = None;
                w.suites[0].latest_check_runs_count = 1;
            },
            &[Reason::GatesNotRun, Reason::SuiteNotSucceeded],
        ),
        (
            "gates-matrix alone, even in a completed suite",
            |w| {
                w.runs.retain(|run| run.id == 1);
                w.suites[0].latest_check_runs_count = 1;
            },
            &[Reason::GatesNotRun],
        ),
        (
            "a check suite that concluded failure",
            |w| w.suites[0].conclusion = Some("failure".to_owned()),
            &[Reason::SuiteNotSucceeded],
        ),
        (
            "the gates-matrix run's check suite is missing",
            |w| w.suites[0].id = 8,
            &[Reason::SuiteMissing],
        ),
        (
            "a gates-matrix run with no check suite",
            |w| run_mut(w, 1).suite = None,
            &[Reason::SuiteMissing],
        ),
        (
            "no check suites at all",
            |w| w.suites.clear(),
            &[Reason::SuiteMissing],
        ),
        (
            "another App's check suite still running",
            |w| {
                let mut other = suite(10, "other-ci", 1);
                other.status = Some("in_progress".to_owned());
                other.conclusion = None;
                w.suites.push(other);
            },
            &[Reason::SuiteNotSucceeded],
        ),
        (
            "a gate run in the gates suite created by another App",
            |w| run_mut(w, 2).app = Some("impostor".to_owned()),
            &[Reason::ForeignGateRun],
        ),
        (
            "an unresolved review thread",
            |w| {
                w.threads.push(ReviewThread {
                    id: "T2".to_owned(),
                    resolved: false,
                })
            },
            &[Reason::UnresolvedThreads],
        ),
        (
            "the PR conflicts",
            |w| w.pull.mergeable_state = Some("dirty".to_owned()),
            &[Reason::Conflict],
        ),
        (
            "mergeable is still null",
            |w| w.pull.mergeable = None,
            &[Reason::MergeabilityUnknown],
        ),
        (
            "mergeable is false",
            |w| w.pull.mergeable = Some(false),
            &[Reason::NotMergeable],
        ),
        ("a draft", |w| w.pull.draft = true, &[Reason::Draft]),
        (
            "a closed PR",
            |w| w.pull.state = "closed".to_owned(),
            &[Reason::NotOpen],
        ),
        (
            "a base other than master",
            |w| w.pull.base = BranchName("release".to_owned()),
            &[Reason::WrongBase],
        ),
        (
            "a head that is not a mintswarm/* branch",
            |w| w.pull.branch = BranchName("feat/568-steward-approvals".to_owned()),
            &[Reason::NotFactoryBranch],
        ),
        (
            "a head in a fork",
            |w| w.pull.head_repo = Some("someone/mintswarm".to_owned()),
            &[Reason::ForkHead],
        ),
        (
            "a deleted head repository",
            |w| w.pull.head_repo = None,
            &[Reason::ForkHead],
        ),
        (
            "the hold label",
            |w| w.pull.labels.push("hold".to_owned()),
            &[Reason::Hold],
        ),
        (
            "no approval at all",
            |w| w.reviews.clear(),
            &[Reason::NotCovered],
        ),
        (
            "no approval groups",
            |w| w.groups.clear(),
            &[Reason::NotCovered],
        ),
        (
            "checks read at another commit",
            |w| w.checks_at = old(),
            &[Reason::ChecksNotAtHead],
        ),
        (
            "no linked issue supplied",
            |w| w.issue = None,
            &[Reason::NoLinkedIssue],
        ),
        (
            "a branch that names no issue",
            |w| w.pull.branch = BranchName("mintswarm/steward-approvals".to_owned()),
            &[Reason::NoLinkedIssue],
        ),
        (
            "a supplied issue with another number",
            |w| {
                if let Some(issue) = w.issue.as_mut() {
                    issue.number = IssueNumber(569);
                }
            },
            &[Reason::NoLinkedIssue],
        ),
        (
            "an untrusted linked issue",
            |w| w.ingest.labels.clear(),
            &[Reason::IssueUntrusted],
        ),
        (
            "a linked issue not awaiting merge",
            |w| w.state = Some("in review".to_owned()),
            &[Reason::IssueNotAwaitingMerge],
        ),
        (
            "a linked issue with no state",
            |w| w.state = None,
            &[Reason::IssueNotAwaitingMerge],
        ),
        (
            "a review requested again from the approver",
            |w| w.requested.users.push(Login("ZMIJ".to_owned())),
            &[Reason::ReviewRerequested],
        ),
        (
            "a review requested again after changes were requested",
            |w| {
                w.reviews
                    .push(review(2, "alice", "CHANGES_REQUESTED", LATER, head()));
                w.requested.users.push(Login("alice".to_owned()));
            },
            &[Reason::ReviewRerequested],
        ),
        (
            "the approve_pr row trusts no one",
            |w| w.approve_pr.people = People::None,
            &[Reason::ApprovalsNotTrusted],
        ),
    ];
    for (name, change, expected) in cases {
        let outcome = outcome(change);
        assert!(!outcome.merge_ready, "{name}: merge ready");
        for reason in *expected {
            assert!(
                outcome.reasons.contains(reason),
                "{name}: {reason:?} missing from {:?}",
                outcome.reasons
            );
        }
    }
}

#[test]
fn reasons_are_kept_once_in_guard_order() {
    let outcome = outcome(|w| {
        w.reviews.clear();
        w.pull.draft = true;
        w.issue = None;
        run_mut(w, 2).conclusion = Some("failure".to_owned());
        run_mut(w, 3).conclusion = Some("failure".to_owned());
        w.statuses.statuses.push(status("ci/a", None));
        w.statuses.statuses.push(status("ci/b", None));
        w.statuses.total_count = 3;
        w.statuses.statuses[0].conclusion = None;
    });
    assert_eq!(
        outcome.reasons,
        [
            Reason::NoLinkedIssue,
            Reason::Draft,
            Reason::RunNotSucceeded,
            Reason::StatusNotSucceeded,
            Reason::NotCovered,
        ]
    );
}

#[test]
fn a_first_review_request_does_not_block() {
    assert_ready(&outcome(|w| {
        w.requested.users.push(Login("alice".to_owned()))
    }));
}

#[test]
fn a_team_request_does_not_block() {
    assert_ready(&outcome(|w| w.requested.teams.push("core".to_owned())));
}

#[test]
fn a_commented_review_makes_a_request_no_rerequest() {
    assert_ready(&outcome(|w| {
        w.reviews
            .push(review(2, "alice", "COMMENTED", LATER, head()));
        w.requested.users.push(Login("alice".to_owned()));
    }));
}

#[test]
fn linked_issue_reads_factory_branches_only() {
    let read = |name: &str| linked_issue(&BranchName(name.to_owned()));
    assert_eq!(
        read("mintswarm/568-steward-approvals"),
        Some(IssueNumber(568))
    );
    assert_eq!(read("mintswarm/568-"), None);
    assert_eq!(read("mintswarm/-slug"), None);
    assert_eq!(read("mintswarm/56a-slug"), None);
    assert_eq!(read("mintswarm/568"), None);
    assert_eq!(read("feat/568-steward-approvals"), None);
}

#[test]
fn passing_world_is_merge_ready() {
    let world = passing();
    let outcome = evaluate(&facts(&world));
    assert_ready(&outcome);
    assert_eq!(outcome.head, head());
    assert_eq!(outcome.approvals, world.reviews);
    assert!(outcome.hints.is_empty());
}

#[test]
fn two_groups_two_owners_merge_ready() {
    let mut world = passing();
    world.groups = vec![group(1, "crates/", OWNER), group(2, "web/", "alice-dev")];
    world
        .reviews
        .push(review(2, "Alice-Dev", "APPROVED", LATER, head()));
    world
        .roles
        .insert(Login("Alice-Dev".to_owned()), RepoRole::Write);
    let outcome = evaluate(&facts(&world));
    assert_ready(&outcome);
    assert_eq!(outcome.head, head());
    assert_eq!(outcome.approvals, world.reviews);
    assert_eq!(outcome.cover.satisfied, [0, 1]);
}

#[test]
fn one_of_two_groups_is_not_covered() {
    let outcome = outcome(|w| {
        w.groups = vec![group(1, "crates/", OWNER), group(2, "web/", "alice-dev")];
    });
    assert_refused(&outcome, &[Reason::NotCovered]);
    assert!(!outcome.reasons.contains(&Reason::ShapeChanged));
    assert_eq!(outcome.cover.open, [1]);
}

#[test]
fn stale_commit_without_shape_fact_does_not_count() {
    let outcome = outcome(|w| w.reviews[0].commit = Some(old()));
    assert_refused(&outcome, &[Reason::NotCovered, Reason::ShapeChanged]);
    assert!(outcome.approvals.is_empty());
}

#[test]
fn stale_commit_with_other_shape_fact_does_not_count() {
    let outcome = outcome(|w| {
        w.reviews[0].commit = Some(old());
        w.same_shape = vec![sha('d')];
    });
    assert_refused(&outcome, &[Reason::NotCovered, Reason::ShapeChanged]);
    assert!(outcome.approvals.is_empty());
}

#[test]
fn stale_commit_with_same_shape_counts() {
    let mut world = passing();
    world.reviews[0].commit = Some(old());
    world.same_shape = vec![old()];
    let outcome = evaluate(&facts(&world));
    assert_ready(&outcome);
    assert_eq!(outcome.approvals, world.reviews);
}

#[test]
fn approval_anchor_moved_to_head_counts() {
    // GitHub moved the approval's anchor to the new head on a clean update of the branch.
    let mut world = passing();
    world.pull.head = sha('c');
    world.checks_at = sha('c');
    world.reviews[0].commit = Some(sha('c'));
    let outcome = evaluate(&facts(&world));
    assert_ready(&outcome);
    assert_eq!(outcome.head, sha('c'));
    assert_eq!(outcome.approvals, world.reviews);
}

#[test]
fn approval_with_no_commit_does_not_count() {
    let outcome = outcome(|w| w.reviews[0].commit = None);
    assert_refused(&outcome, &[Reason::NotCovered, Reason::ShapeChanged]);
}

#[test]
fn bot_approval_does_not_count() {
    let outcome = outcome(|w| {
        w.reviews = vec![review(1, BOT, "APPROVED", APPROVED_AT, head())];
        w.roles.insert(Login(BOT.to_owned()), RepoRole::Admin);
        w.groups = vec![ApprovalGroup {
            key: GroupKey::Unowned,
            eligible: Eligibility::AnyoneWithWriteAccess,
            paths: vec!["file.rs".to_owned()],
        }];
    });
    assert_refused(&outcome, &[Reason::NotCovered]);
    assert!(outcome.approvals.is_empty());
}

#[test]
fn pending_review_takes_no_part() {
    let mut world = passing();
    let mut pending = review(2, OWNER, "PENDING", LATER, head());
    pending.submitted_at = None;
    world.reviews.push(pending);
    let outcome = evaluate(&facts(&world));
    assert_ready(&outcome);
    assert_eq!(outcome.approvals, world.reviews[..1]);
}

#[test]
fn old_steer_form_naming_head_grants_nothing() {
    let outcome = outcome(|w| {
        w.reviews.clear();
        w.comments = vec![comment(10, BOT, &old_steer(&head()), LATER, LATER)];
    });
    assert_refused(&outcome, &[Reason::NotCovered]);
    assert!(outcome.approvals.is_empty());
}

#[test]
fn old_steer_form_naming_other_sha_grants_nothing() {
    let outcome = outcome(|w| {
        w.reviews.clear();
        w.comments = vec![comment(10, BOT, &old_steer(&old()), LATER, LATER)];
    });
    assert_refused(&outcome, &[Reason::NotCovered]);
    assert!(outcome.approvals.is_empty());
}

#[test]
fn old_steer_form_withdraws_nothing() {
    assert_ready(&outcome(|w| {
        w.comments = vec![comment(10, BOT, &old_steer(&old()), LATER, LATER)];
    }));
}

#[test]
fn later_comment_withdraws_nothing() {
    assert_ready(&outcome(|w| {
        w.comments = vec![comment(10, OWNER, "One nit: rename this.", LATER, LATER)];
    }));
}

#[test]
fn later_inline_comment_review_withdraws_nothing() {
    // An inline comment arrives as a COMMENTED review by its author.
    assert_ready(&outcome(|w| {
        w.reviews
            .push(review(2, "alice", "COMMENTED", LATER, head()));
    }));
}

#[test]
fn edit_after_approval_withdraws_nothing() {
    assert_ready(&outcome(|w| {
        w.comments = vec![comment(10, OWNER, "Edited later.", EARLIER, LATER)];
    }));
}

#[test]
fn self_steer_comment_withdraws_nothing() {
    assert_ready(&outcome(|w| {
        let body = format!("{STEER_MARKER}\nThe maintainer asks for a follow-up issue.");
        w.comments = vec![comment(10, BOT, &body, LATER, LATER)];
    }));
}

#[test]
fn other_persons_approval_withdraws_nothing() {
    let mut world = passing();
    world
        .reviews
        .push(review(2, "alice", "APPROVED", LATER, head()));
    world
        .roles
        .insert(Login("alice".to_owned()), RepoRole::Write);
    let outcome = evaluate(&facts(&world));
    assert_ready(&outcome);
    assert_eq!(outcome.approvals, world.reviews);
}

#[test]
fn other_persons_request_changes_does_not_block() {
    let mut world = passing();
    world
        .reviews
        .push(review(2, "alice", "CHANGES_REQUESTED", LATER, head()));
    let outcome = evaluate(&facts(&world));
    assert_ready(&outcome);
    assert_eq!(outcome.approvals, world.reviews[..1]);
}

#[test]
fn own_request_changes_withdraws() {
    let outcome = outcome(|w| {
        w.reviews
            .push(review(2, "ZMIJ", "CHANGES_REQUESTED", LATER, head()));
    });
    assert_refused(&outcome, &[Reason::NotCovered]);
    assert!(!outcome.reasons.contains(&Reason::ShapeChanged));
    assert!(outcome.approvals.is_empty());
}

#[test]
fn dismissal_withdraws() {
    let outcome = outcome(|w| w.reviews[0].state = "DISMISSED".to_owned());
    assert_refused(&outcome, &[Reason::NotCovered]);
    assert!(outcome.approvals.is_empty());
}

#[test]
fn reapproval_after_request_changes_counts() {
    let mut world = passing();
    world.reviews[0].submitted_at = Some(EARLIER.to_owned());
    world.reviews[0].state = "CHANGES_REQUESTED".to_owned();
    world
        .reviews
        .push(review(2, OWNER, "APPROVED", LATER, head()));
    let outcome = evaluate(&facts(&world));
    assert_ready(&outcome);
    assert_eq!(outcome.approvals, world.reviews[1..]);
}

#[test]
fn latest_review_wins_whatever_the_host_order() {
    let outcome = outcome(|w| {
        w.reviews
            .insert(0, review(2, OWNER, "CHANGES_REQUESTED", LATER, head()));
    });
    assert_refused(&outcome, &[Reason::NotCovered]);
}

#[test]
fn own_comment_review_changes_nothing() {
    let mut world = passing();
    world
        .reviews
        .push(review(2, OWNER, "COMMENTED", LATER, head()));
    let outcome = evaluate(&facts(&world));
    assert_ready(&outcome);
    assert_eq!(outcome.approvals, world.reviews[..1]);
}

#[test]
fn approve_comment_yields_hint() {
    let outcome = outcome(|w| {
        w.reviews.clear();
        w.comments = vec![comment(10, OWNER, "approve", LATER, LATER)];
    });
    assert_refused(&outcome, &[Reason::NotCovered]);
    assert!(outcome.approvals.is_empty());
    assert_eq!(
        outcome.hints,
        [ApproveHint {
            comment: 10,
            review_url: format!("{PR_URL}/files"),
        }]
    );
}

#[test]
fn approve_comment_beside_approval_withdraws_nothing() {
    let outcome = outcome(|w| {
        w.comments = vec![comment(10, OWNER, "Approve", LATER, LATER)];
    });
    assert_ready(&outcome);
    assert_eq!(outcome.hints.len(), 1);
}

#[test]
fn bot_approve_comment_yields_no_hint() {
    let outcome = outcome(|w| {
        w.comments = vec![comment(10, BOT, "approve", LATER, LATER)];
    });
    assert_ready(&outcome);
    assert!(outcome.hints.is_empty());
}

#[test]
fn other_commands_and_near_misses_yield_no_hint() {
    let outcome = outcome(|w| {
        w.comments = vec![
            comment(10, OWNER, "reject", LATER, LATER),
            comment(11, OWNER, "aprove", LATER, LATER),
            comment(12, OWNER, "> approve", LATER, LATER),
        ];
    });
    assert_ready(&outcome);
    assert!(outcome.hints.is_empty());
}
