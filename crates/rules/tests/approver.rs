//! The approver rules: the owner walk over the cases of `assign-approvers.py`'s selftest
//! (`.mintswarm/dogfood/github/selftest.sh`, seeds 71 to 78) and a few more, the approver
//! check, and the CODEOWNERS approval-group cover.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use std::collections::BTreeMap;

use serde_json::Value;

use mintswarm_domain::approval;
use mintswarm_domain::codeowners;
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{Login, ParentIssue, RepoRole, is_bot};
use mintswarm_rules::approver::{
    Approver, NotApprover, OwnerError, OwnerStep, OwnerWalk, cover, issue_approver,
};

const OWNER_CASES: &str = include_str!("fixtures/issue_owner.json");

/// A seeded issue: its author with the bot flag, and its parent.
struct Seeded {
    author: Option<Login>,
    author_is_bot: bool,
    parent: Option<u64>,
}

fn seeded(fixture: &Value) -> BTreeMap<u64, Seeded> {
    fixture["issues"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(number, entry)| {
            let login = entry["user"]["login"].as_str();
            let flag = entry["author_is_bot"].as_bool().unwrap_or_else(|| {
                login.is_some_and(|login| is_bot(login, entry["user"]["type"].as_str()))
            });
            let seeded = Seeded {
                author: login.map(|login| Login(login.to_owned())),
                author_is_bot: flag,
                parent: entry["parent"].as_u64(),
            };
            (number.parse().unwrap(), seeded)
        })
        .collect()
}

/// The answer a case expects.
fn expected(case: &Value) -> Result<OwnerStep, OwnerError> {
    let start = IssueNumber(case["start"].as_u64().unwrap());
    let number = |key: &str| IssueNumber(case[key].as_u64().unwrap());
    match case["error"].as_str() {
        None => Ok(OwnerStep::Owner {
            owner: Login(case["owner"].as_str().unwrap().to_owned()),
            from: number("from"),
        }),
        Some("no_person_ancestor") => Err(OwnerError::NoPersonAncestor { issue: start }),
        Some("loop") => Err(OwnerError::Loop {
            issue: start,
            at: number("at"),
        }),
        Some("invalid_login") => Err(OwnerError::InvalidLogin {
            from: number("from"),
            login: Login("not_a_login".to_owned()),
        }),
        Some(other) => panic!("unknown error {other}"),
    }
}

fn walk(issues: &BTreeMap<u64, Seeded>, start: u64) -> Result<OwnerStep, OwnerError> {
    let first = &issues[&start];
    let (mut walk, mut step) = OwnerWalk::start(
        IssueNumber(start),
        first.author.as_ref().unwrap(),
        first.author_is_bot,
    );
    let mut reads = 0;
    while let Ok(OwnerStep::ReadParent(IssueNumber(at))) = step {
        reads += 1;
        assert!(reads < 10, "the walk from #{start} does not end");
        let parent = issues[&at].parent.map(|number| ParentIssue {
            number: IssueNumber(number),
            author: issues[&number].author.clone(),
            author_is_bot: issues[&number].author_is_bot,
        });
        step = walk.parent(parent.as_ref());
    }
    step
}

#[test]
fn the_owner_walk_matches_every_case() {
    let fixture: Value = serde_json::from_str(OWNER_CASES).unwrap();
    let issues = seeded(&fixture);
    for case in fixture["cases"].as_array().unwrap() {
        let start = case["start"].as_u64().unwrap();
        assert_eq!(walk(&issues, start), expected(case), "#{start}");
    }
}

#[test]
fn a_finished_walk_answers_the_same_again() {
    let (mut walk, first) = OwnerWalk::start(IssueNumber(1), &Login("alice".into()), false);
    assert_eq!(walk.parent(None), first);
    let (mut walk, _) = OwnerWalk::start(IssueNumber(1), &Login("ci[bot]".into()), false);
    let ended = walk.parent(None);
    assert_eq!(
        ended,
        Err(OwnerError::NoPersonAncestor {
            issue: IssueNumber(1)
        })
    );
    let person = ParentIssue {
        number: IssueNumber(2),
        author: Some(Login("alice".into())),
        author_is_bot: false,
    };
    assert_eq!(walk.parent(Some(&person)), ended);
}

fn login(name: &str) -> Login {
    Login(name.to_owned())
}

#[test]
fn only_the_owner_with_write_access_is_the_approver() {
    let owner = login("Alice");
    for role in [RepoRole::Write, RepoRole::Maintain, RepoRole::Admin] {
        assert_eq!(issue_approver(&login("alice"), false, &owner, role), Ok(()));
        assert_eq!(issue_approver(&login("ALICE"), false, &owner, role), Ok(()));
    }
    for role in [RepoRole::Triage, RepoRole::Read, RepoRole::NoAccess] {
        assert_eq!(
            issue_approver(&login("alice"), false, &owner, role),
            Err(NotApprover::NoWriteAccess(role))
        );
    }
    assert_eq!(
        issue_approver(&login("bob"), false, &owner, RepoRole::Admin),
        Err(NotApprover::NotOwner)
    );
}

#[test]
fn a_bot_is_never_the_approver() {
    for (sender, flag) in [
        ("alice[bot]", false),
        ("alice[BOT]", false),
        ("alice", true),
    ] {
        let owner = login(sender.trim_end_matches("[bot]").trim_end_matches("[BOT]"));
        assert_eq!(
            issue_approver(&login(sender), flag, &owner, RepoRole::Admin),
            Err(NotApprover::Bot),
            "{sender}"
        );
    }
    assert_eq!(
        issue_approver(&login("ci[bot]"), false, &login("ci[bot]"), RepoRole::Admin),
        Err(NotApprover::Bot)
    );
}

const CODEOWNERS: &str = "/a/ @alice\n/b/ @bob\n/c/ @ci[bot]\n";

fn groups(paths: &[&str]) -> Vec<approval::ApprovalGroup> {
    let parsed = codeowners::parse(CODEOWNERS);
    assert_eq!(parsed.errors, []);
    approval::groups(&parsed.rules, paths)
}

fn person(name: &str) -> Approver {
    Approver {
        login: login(name),
        is_bot: false,
    }
}

fn roles(entries: &[(&str, RepoRole)]) -> BTreeMap<Login, RepoRole> {
    entries
        .iter()
        .map(|(name, role)| (login(name), *role))
        .collect()
}

#[test]
fn two_groups_are_covered_only_when_each_has_an_approving_owner() {
    let groups = groups(&["a/x.rs", "b/y.rs"]);
    assert_eq!(groups.len(), 2);
    let writers = roles(&[("alice", RepoRole::Write), ("bob", RepoRole::Maintain)]);

    let alone = cover(&groups, &[person("alice")], &writers);
    assert!(!alone.covered);
    assert_eq!((alone.satisfied, alone.open), (vec![0], vec![1]));

    let both = cover(&groups, &[person("alice"), person("bob")], &writers);
    assert!(both.covered);
    assert_eq!(both.open, Vec::<usize>::new());

    let bob_reads = roles(&[("alice", RepoRole::Write), ("bob", RepoRole::Triage)]);
    assert!(!cover(&groups, &[person("alice"), person("bob")], &bob_reads).covered);
    // A login missing from the roles has no access.
    let only_alice = roles(&[("alice", RepoRole::Write)]);
    assert!(!cover(&groups, &[person("alice"), person("bob")], &only_alice).covered);
}

#[test]
fn a_bot_approver_covers_no_group() {
    let groups = groups(&["a/x.rs", "c/z.rs"]);
    let writers = roles(&[("alice", RepoRole::Write), ("ci[bot]", RepoRole::Write)]);
    for is_bot in [false, true] {
        let bot = Approver {
            login: login("ci[bot]"),
            is_bot,
        };
        let cover = cover(&groups, &[person("alice"), bot], &writers);
        assert!(!cover.covered, "flagged: {is_bot}");
        assert_eq!(cover.open, [1]);
    }
}

#[test]
fn no_changed_paths_is_never_covered() {
    let writers = roles(&[("alice", RepoRole::Write)]);
    assert!(!cover(&groups(&[]), &[person("alice")], &writers).covered);
}
