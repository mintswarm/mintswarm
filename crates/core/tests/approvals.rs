//! The approver reads through the in-memory tracker: the owner walk over the ported
//! `assign-approvers.py` cases, the approver check, and the CODEOWNERS approval-group cover at a
//! pinned base.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use mintswarm_core::approvals::approver::{ApproverError, issue_approver, issue_owner, pr_cover};
use mintswarm_core::testing::tracker::{FakeTracker, FakeTrackerError};
use mintswarm_domain::ids::{IssueNumber, PrNumber, RevId};
use mintswarm_domain::tracker::{
    ChangedFile, Issue, Issues, Link, LinkKind, Login, PullDetail, RepoRole, is_bot,
};
use mintswarm_domain::vcs::BranchName;
use mintswarm_rules::approver::{Approver, NotApprover, OwnerError};
use serde_json::Value;

/// The owner cases the rules crate's walk is tested on, shared so both crates check the same.
const OWNER_CASES: &str = include_str!("../../rules/tests/fixtures/issue_owner.json");
const BOT: &str = "mintswarm-dev-keeper[bot]";

fn login(name: &str) -> Login {
    Login(name.to_owned())
}

fn issue(number: u64, author: &str, author_is_bot: bool) -> Issue {
    Issue {
        number: IssueNumber(number),
        title: format!("Issue {number}"),
        body: String::new(),
        state: "open".into(),
        labels: vec!["mintswarm".into()],
        issue_type: None,
        author: login(author),
        author_is_bot,
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        sub_issues: 0,
        blocked_by: 0,
    }
}

/// The fake seeded with every fixture issue that names an author, and the parent links between
/// them, with bot flags from [`is_bot`] unless the fixture sets one.
fn seeded(fixture: &Value) -> FakeTracker {
    let fake = FakeTracker::new(login(BOT));
    let issues = fixture["issues"].as_object().unwrap();
    let seedable = |number: &str| issues[number]["user"]["login"].is_string();
    for (number, entry) in issues {
        if !seedable(number) {
            continue;
        }
        let author = entry["user"]["login"].as_str().unwrap();
        let flag = entry["author_is_bot"]
            .as_bool()
            .unwrap_or_else(|| is_bot(author, entry["user"]["type"].as_str()));
        let number: u64 = number.parse().unwrap();
        fake.add_issue(issue(number, author, flag));
        if let Some(parent) = entry["parent"].as_u64()
            && seedable(&parent.to_string())
        {
            fake.set_links(
                &IssueNumber(number),
                vec![Link {
                    kind: LinkKind::Parent,
                    number: parent,
                }],
            );
        }
    }
    fake
}

async fn owner_of(
    fake: &FakeTracker,
    number: u64,
) -> Result<Login, ApproverError<FakeTrackerError>> {
    let read = fake.read_issue(IssueNumber(number), None).await.unwrap();
    issue_owner(fake, &read.issue).await
}

#[tokio::test]
async fn the_owner_is_found_for_every_case_the_fake_can_hold() {
    let fixture: Value = serde_json::from_str(OWNER_CASES).unwrap();
    let fake = seeded(&fixture);
    for case in fixture["cases"].as_array().unwrap() {
        if case["fake"] == false {
            continue;
        }
        let start = case["start"].as_u64().unwrap();
        let got = owner_of(&fake, start).await;
        let number = |key: &str| IssueNumber(case[key].as_u64().unwrap());
        match case["error"].as_str() {
            None => assert_eq!(
                got.unwrap(),
                login(case["owner"].as_str().unwrap()),
                "#{start}"
            ),
            Some(kind) => {
                let expected = match kind {
                    "no_person_ancestor" => OwnerError::NoPersonAncestor {
                        issue: IssueNumber(start),
                    },
                    "loop" => OwnerError::Loop {
                        issue: IssueNumber(start),
                        at: number("at"),
                    },
                    "invalid_login" => OwnerError::InvalidLogin {
                        from: number("from"),
                        login: login("not_a_login"),
                    },
                    other => panic!("unknown error {other}"),
                };
                assert!(
                    matches!(&got, Err(ApproverError::Owner(error)) if *error == expected),
                    "#{start}: {got:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_parent_read_failure_is_the_trackers_error() {
    let fake = FakeTracker::new(login(BOT));
    fake.add_issue(issue(1, BOT, true));
    fake.set_links(
        &IssueNumber(1),
        vec![Link {
            kind: LinkKind::Parent,
            number: 2,
        }],
    );
    let got = owner_of(&fake, 1).await;
    assert!(matches!(got, Err(ApproverError::Tracker(_))), "{got:?}");
}

#[tokio::test]
async fn the_approver_is_the_owner_with_write_access_and_no_bot() {
    let fake = FakeTracker::new(login(BOT));
    let filed = issue(71, "alice", false);
    fake.add_issue(filed.clone());
    fake.set_role(login("alice"), RepoRole::Write);
    fake.set_role(login("triager"), RepoRole::Triage);
    fake.set_role(login("reader"), RepoRole::Read);
    fake.set_role(login(BOT), RepoRole::Admin);

    let answer = |sender: &'static str, is_bot: bool| {
        let fake = &fake;
        let filed = &filed;
        async move {
            issue_approver(fake, filed, &login(sender), is_bot)
                .await
                .unwrap()
        }
    };
    assert_eq!(answer("alice", false).await, Ok(()));
    assert_eq!(answer("bob", false).await, Err(NotApprover::NotOwner));
    assert_eq!(answer("alice", true).await, Err(NotApprover::Bot));
    assert_eq!(answer(BOT, false).await, Err(NotApprover::Bot));

    for (number, name, role) in [
        (72, "triager", RepoRole::Triage),
        (73, "reader", RepoRole::Read),
    ] {
        let owned = issue(number, name, false);
        fake.add_issue(owned.clone());
        assert_eq!(
            issue_approver(&fake, &owned, &login(name), false)
                .await
                .unwrap(),
            Err(NotApprover::NoWriteAccess(role)),
            "{name}"
        );
    }
}

const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
const PR: PrNumber = PrNumber(400);

fn pull() -> PullDetail {
    PullDetail {
        number: PR,
        url: "https://github.com/acme/widgets/pull/400".into(),
        title: "feat(domain): approvers".into(),
        state: "open".into(),
        draft: false,
        base: BranchName("master".into()),
        branch: BranchName("mintswarm/560-approvers".into()),
        head: RevId("89abcdef0123456789abcdef0123456789abcdef".into()),
        head_repo: Some("acme/widgets".into()),
        labels: Vec::new(),
        mergeable: Some(true),
        mergeable_state: Some("clean".into()),
        merged: false,
        merge_commit: None,
        changed_files: 2,
    }
}

fn changed(path: &str, previous: Option<&str>) -> ChangedFile {
    ChangedFile {
        path: path.to_owned(),
        previous_path: previous.map(str::to_owned),
        status: if previous.is_some() {
            "renamed"
        } else {
            "modified"
        }
        .into(),
    }
}

/// A fake with the CODEOWNERS file at the base, `files` changed by the pull request, and
/// alice and the bot writers while bob only triages.
fn with_pull(files: Vec<ChangedFile>) -> FakeTracker {
    let fake = FakeTracker::new(login(BOT));
    fake.add_pull(pull());
    fake.put_file(
        &RevId(BASE.into()),
        ".github/CODEOWNERS",
        "/a/ @alice\n/b/ @bob\n/c/ @ci[bot]\n",
    );
    fake.set_changed_files(&PR, files);
    fake.set_role(login("alice"), RepoRole::Write);
    fake.set_role(login("bob"), RepoRole::Triage);
    fake.set_role(login("ci[bot]"), RepoRole::Write);
    fake
}

fn person(name: &str) -> Approver {
    Approver {
        login: login(name),
        is_bot: false,
    }
}

async fn covered(fake: &FakeTracker, approvers: &[Approver]) -> bool {
    pr_cover(fake, PR, RevId(BASE.into()), approvers)
        .await
        .unwrap()
        .cover
        .covered
}

#[tokio::test]
async fn two_groups_are_covered_only_when_each_has_an_approving_owner() {
    // The rename's previous path falls in the `/b/` group.
    let fake = with_pull(vec![
        changed("a/x.rs", None),
        changed("a/moved.rs", Some("b/y.rs")),
    ]);
    let read = pr_cover(&fake, PR, RevId(BASE.into()), &[person("alice")])
        .await
        .unwrap();
    assert_eq!(read.groups.len(), 2);
    assert_eq!(read.line_errors, []);
    assert_eq!(read.cover.open, [1]);
    assert!(!read.cover.covered);

    // bob only triages, so his approval does not count.
    assert!(!covered(&fake, &[person("alice"), person("bob")]).await);
    fake.set_role(login("bob"), RepoRole::Write);
    assert!(covered(&fake, &[person("alice"), person("bob")]).await);
}

#[tokio::test]
async fn a_bot_approver_covers_no_group() {
    let fake = with_pull(vec![changed("a/x.rs", None), changed("c/z.rs", None)]);
    for is_bot in [false, true] {
        let bot = Approver {
            login: login("ci[bot]"),
            is_bot,
        };
        assert!(!covered(&fake, &[person("alice"), bot]).await, "{is_bot}");
    }
}

#[tokio::test]
async fn no_changed_paths_is_never_covered() {
    let fake = with_pull(Vec::new());
    assert!(!covered(&fake, &[person("alice")]).await);
}
