//! Approval groups of a pull request's changed paths, and when its approvals complete it.

#![cfg(test)]

use std::collections::BTreeMap;

use mintswarm_domain::approval::{
    ApprovalGroup, ApprovalStatus, Eligibility, Facts, GroupKey, Protection, evaluate, groups,
};
use mintswarm_domain::codeowners::{Owner, Team, parse};
use mintswarm_domain::tracker::Login;

const CODEOWNERS: &str = "\
* @lead
/crates/ @rust
/docs/ @writer
/web/ @MintSwarm/Web
/apps/ @octocat
/apps/github
";

fn login(name: &str) -> Login {
    Login(name.to_owned())
}

fn logins(names: &[&str]) -> Vec<Login> {
    names.iter().copied().map(login).collect()
}

fn groups_of(text: &str, paths: &[&str]) -> Vec<ApprovalGroup> {
    let parsed = parse(text);
    assert_eq!(parsed.errors, []);
    groups(&parsed.rules, paths)
}

fn required(count: u32) -> Protection {
    Protection {
        require_code_owner_review: true,
        required_approving_reviews: count,
    }
}

fn everyone_writes(_: &Login) -> bool {
    true
}

fn status(groups: &[ApprovalGroup], approvers: &[&str], protection: Protection) -> ApprovalStatus {
    let teams = BTreeMap::from([(Team::new("mintswarm", "web"), logins(&["Wendy"]))]);
    let facts = Facts {
        teams: &teams,
        has_write: &everyone_writes,
    };
    evaluate(groups, &logins(approvers), &facts, &protection)
}

#[test]
fn there_is_one_group_per_matching_rule_in_rule_order() {
    let groups = groups_of(
        CODEOWNERS,
        &[
            "docs/a.md",
            "crates/x/lib.rs",
            "README.md",
            "crates/y/lib.rs",
        ],
    );
    let keys: Vec<&GroupKey> = groups.iter().map(|g| &g.key).collect();
    assert_eq!(
        keys,
        [
            &GroupKey::Rule {
                line: 1,
                pattern: "*".to_owned()
            },
            &GroupKey::Rule {
                line: 2,
                pattern: "/crates/".to_owned()
            },
            &GroupKey::Rule {
                line: 3,
                pattern: "/docs/".to_owned()
            },
        ]
    );
    assert_eq!(groups[1].paths, ["crates/x/lib.rs", "crates/y/lib.rs"]);
    assert_eq!(
        groups[1].eligible,
        Eligibility::Owners(vec![Owner::User(login("rust"))])
    );
}

#[test]
fn rules_with_the_same_owners_keep_their_own_groups() {
    let groups = groups_of("/a/ @x\n/b/ @x\n", &["a/1", "b/1"]);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].eligible, groups[1].eligible);
}

#[test]
fn unowned_paths_form_one_group_for_anyone_with_write_access() {
    // `other/x` matches no rule; `apps/github/a.rb` matches a rule with no owners.
    let groups = groups_of(
        "/apps/ @octocat\n/apps/github\n",
        &["other/x", "apps/github/a.rb", "apps/web/b.rb"],
    );
    let unowned: Vec<&ApprovalGroup> = groups
        .iter()
        .filter(|g| g.key == GroupKey::Unowned)
        .collect();
    assert_eq!(unowned.len(), 1);
    assert_eq!(unowned[0].eligible, Eligibility::AnyoneWithWriteAccess);
    assert_eq!(unowned[0].paths, ["other/x", "apps/github/a.rb"]);
    assert_eq!(groups.last().unwrap().key, GroupKey::Unowned);
}

#[test]
fn no_unowned_group_when_every_path_has_an_owner() {
    let groups = groups_of(CODEOWNERS, &["README.md"]);
    assert!(groups.iter().all(|g| g.key != GroupKey::Unowned));
}

#[test]
fn a_path_given_twice_counts_once() {
    let groups = groups_of(CODEOWNERS, &["docs/a.md", "docs/a.md"]);
    assert_eq!(groups[0].paths, ["docs/a.md"]);
}

#[test]
fn one_open_group_keeps_the_approval_incomplete() {
    let groups = groups_of(CODEOWNERS, &["crates/x/lib.rs", "docs/a.md"]);
    let status = status(&groups, &["rust"], required(1));
    assert_eq!(status.satisfied, [0]);
    assert_eq!(status.open, [1]);
    assert!(!status.complete);
}

#[test]
fn too_few_distinct_approvers_keep_the_approval_incomplete() {
    let groups = groups_of(CODEOWNERS, &["crates/x/lib.rs", "docs/a.md"]);
    let status = status(&groups, &["rust", "writer"], required(3));
    assert_eq!(status.open, Vec::<usize>::new());
    assert_eq!(status.distinct_approvers, 2);
    assert!(!status.complete);
}

#[test]
fn every_group_approved_and_enough_approvers_completes_it() {
    let groups = groups_of(CODEOWNERS, &["crates/x/lib.rs", "docs/a.md"]);
    let status = status(&groups, &["rust", "writer"], required(2));
    assert_eq!(status.satisfied, [0, 1]);
    assert!(status.complete);
}

#[test]
fn a_team_member_satisfies_a_team_group() {
    let groups = groups_of(CODEOWNERS, &["web/src/app.ts"]);
    assert!(status(&groups, &["wendy"], required(1)).complete);
    assert!(!status(&groups, &["rust"], required(1)).complete);
}

#[test]
fn a_team_missing_from_the_facts_has_no_members() {
    let groups = groups_of("/x/ @org/unknown\n", &["x/a"]);
    let status = status(&groups, &["wendy"], required(1));
    assert_eq!(status.open, [0]);
    assert!(!status.complete);
}

#[test]
fn an_email_owner_cannot_be_satisfied() {
    let groups = groups_of("/x/ docs@example.com\n", &["x/a"]);
    assert_eq!(status(&groups, &["docs"], required(1)).open, [0]);
}

#[test]
fn logins_are_compared_without_case_and_counted_once() {
    let groups = groups_of(CODEOWNERS, &["crates/x/lib.rs"]);
    let status = status(&groups, &["RUST", "rust", "Rust"], required(2));
    assert_eq!(status.satisfied, [0]);
    assert_eq!(status.distinct_approvers, 1);
    assert!(!status.complete);
}

#[test]
fn an_approver_without_write_access_does_not_count() {
    let groups = groups_of(CODEOWNERS, &["crates/x/lib.rs", "other/x"]);
    let teams = BTreeMap::new();
    let writes = |l: &Login| l.0 != "rust";
    let facts = Facts {
        teams: &teams,
        has_write: &writes,
    };
    let status = evaluate(&groups, &logins(&["rust"]), &facts, &required(1));
    assert_eq!(status.distinct_approvers, 0);
    assert_eq!(status.satisfied, Vec::<usize>::new());
    assert!(!status.complete);
}

#[test]
fn anyone_with_write_access_satisfies_the_unowned_group() {
    let groups = groups_of("/apps/ @octocat\n/apps/github\n", &["apps/github/a.rb"]);
    let status = status(&groups, &["someone"], required(1));
    assert_eq!(status.satisfied, [0]);
    assert!(status.complete);
}

#[test]
fn without_code_owner_review_only_the_count_decides() {
    let groups = groups_of(CODEOWNERS, &["crates/x/lib.rs", "docs/a.md"]);
    let protection = Protection {
        require_code_owner_review: false,
        required_approving_reviews: 1,
    };
    let status = status(&groups, &["someone"], protection);
    assert_eq!(status.open, [0, 1]);
    assert!(status.complete);
    let none = self::status(&groups, &[], protection);
    assert!(!none.complete);
}

#[test]
fn the_item_owner_who_is_no_code_owner_leaves_the_group_open() {
    // `evaluate` takes no owner: the item's owner approving counts only as a code owner would.
    let groups = groups_of(CODEOWNERS, &["crates/x/lib.rs"]);
    let item_owner = "author";
    let status = status(&groups, &[item_owner], required(1));
    assert_eq!(status.open, [0]);
    assert!(!status.complete);
}

#[test]
fn groups_serialise_in_snake_case() {
    let groups = groups_of("/apps/ @octocat\n/apps/github\n", &["apps/github/a.rb"]);
    let json = serde_json::to_value(&groups).unwrap();
    assert_eq!(
        json,
        serde_json::json!([{
            "key": "unowned",
            "eligible": "anyone_with_write_access",
            "paths": ["apps/github/a.rb"],
        }])
    );
}
