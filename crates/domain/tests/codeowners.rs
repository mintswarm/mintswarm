//! CODEOWNERS parsing and matching, against GitHub's documented rules and the rules of
//! `assign-approvers.py`.

#![cfg(test)]

use mintswarm_domain::codeowners::{CodeOwners, LineErrorKind, Owner, Team, parse};
use mintswarm_domain::tracker::Login;

fn rules(text: &str) -> CodeOwners {
    let parsed = parse(text);
    assert_eq!(parsed.errors, [], "the text parses cleanly");
    parsed.rules
}

/// The line of the rule that owns PATH, or `None`.
fn line(owners: &CodeOwners, path: &str) -> Option<usize> {
    owners.matching_rule(path).map(|rule| rule.line)
}

/// Whether the one-rule file PATTERN matches PATH.
fn matches(pattern: &str, path: &str) -> bool {
    line(&rules(&format!("{pattern} @a")), path).is_some()
}

fn user(login: &str) -> Owner {
    Owner::User(Login(login.to_owned()))
}

#[test]
fn the_last_matching_rule_wins() {
    let owners = rules("* @all\n*.rs @rust\n/crates/ @crates\n");
    assert_eq!(line(&owners, "README.md"), Some(1));
    assert_eq!(line(&owners, "src/main.rs"), Some(2));
    assert_eq!(line(&owners, "crates/domain/src/lib.rs"), Some(3));
    assert_eq!(line(&owners, "crates/README.md"), Some(3));
}

#[test]
fn an_earlier_rule_does_not_win_when_a_later_one_matches() {
    let owners = rules("/crates/ @crates\n* @all\n");
    assert_eq!(line(&owners, "crates/domain/src/lib.rs"), Some(2));
}

#[test]
fn no_rule_matching_gives_none() {
    let owners = rules("/docs/ @docs\n");
    assert_eq!(owners.matching_rule("src/lib.rs"), None);
}

#[test]
fn a_leading_slash_anchors_to_the_root() {
    assert!(matches("/build", "build"));
    assert!(matches("/build", "build/out.txt"));
    assert!(!matches("/build", "crates/build"));
    assert!(!matches("/build", "crates/build/out.txt"));
}

#[test]
fn an_inner_slash_anchors_to_the_root() {
    assert!(matches("crates/core", "crates/core/src/lib.rs"));
    assert!(!matches("crates/core", "vendor/crates/core/src/lib.rs"));
}

#[test]
fn a_pattern_without_a_slash_matches_at_any_depth() {
    assert!(matches("Makefile", "Makefile"));
    assert!(matches("Makefile", "a/b/Makefile"));
    assert!(matches("logs", "deeply/nested/logs/today.txt"));
    assert!(!matches("Makefile", "Makefile.am"));
    assert!(!matches("Makefile", "a/GNUmakefile"));
}

#[test]
fn a_pattern_matches_a_file_or_anything_under_a_directory() {
    assert!(matches("/docs", "docs"));
    assert!(matches("/docs", "docs/a/b.md"));
    assert!(!matches("/docs", "docs.md"));
}

#[test]
fn a_trailing_slash_matches_only_under_a_directory() {
    // GitHub: `apps/` owns any file in an apps directory anywhere in the repository.
    assert!(matches("apps/", "apps/web/index.ts"));
    assert!(matches("apps/", "services/apps/main.go"));
    assert!(!matches("apps/", "apps"));
    // GitHub: `/build/logs/` and `/docs/` own their directories at the root, and everything
    // below them.
    assert!(matches("/build/logs/", "build/logs/a.log"));
    assert!(matches("/build/logs/", "build/logs/2026/a.log"));
    assert!(!matches("/build/logs/", "x/build/logs/a.log"));
    assert!(!matches("/build/logs/", "build/logs"));
    assert!(matches("/docs/", "docs/sub/page.md"));
}

#[test]
fn star_matches_within_one_segment() {
    // GitHub: `*.js` owns every JavaScript file, at any depth.
    assert!(matches("*.js", "app.js"));
    assert!(matches("*.js", "web/src/app.js"));
    assert!(!matches("*.js", "app.json"));
    assert!(matches("/src/*.rs", "src/lib.rs"));
    assert!(!matches("/src/*.rs", "src/a/lib.rs"));
}

#[test]
fn a_lone_star_matches_everything() {
    // GitHub: `*` sets the default owners for everything in the repository.
    assert!(matches("*", "README.md"));
    assert!(matches("*", "a/b/c/d.txt"));
}

#[test]
fn an_anchored_star_owns_only_its_segment() {
    // GitHub: `docs/*` matches `docs/getting-started.md` but not
    // `docs/build-app/troubleshooting.md`.
    assert!(matches("docs/*", "docs/getting-started.md"));
    assert!(!matches("docs/*", "docs/build-app/troubleshooting.md"));
    assert!(matches("/docs/*", "docs/a.md"));
    assert!(!matches("/docs/*", "docs/b/c.md"));
    assert!(!matches("docs/*", "x/docs/a.md"));
}

#[test]
fn question_mark_matches_one_character_other_than_slash() {
    assert!(matches("/v?.txt", "v1.txt"));
    assert!(!matches("/v?.txt", "v10.txt"));
    assert!(!matches("/a?b", "a/b"));
}

#[test]
fn double_star_slash_matches_zero_or_more_directories() {
    // GitHub: `**/logs` owns any file in a logs directory, such as `/build/logs`,
    // `/scripts/logs` and `/deeply/nested/logs`.
    assert!(matches("**/logs", "build/logs/a.txt"));
    assert!(matches("**/logs", "scripts/logs/a.txt"));
    assert!(matches("**/logs", "deeply/nested/logs/a.txt"));
    assert!(matches("**/logs", "logs/a.txt"));
    assert!(matches("/a/**/b", "a/b"));
    assert!(matches("/a/**/b", "a/x/y/b"));
    assert!(!matches("/a/**/b", "a/xb"));
}

#[test]
fn a_trailing_double_star_matches_everything_below() {
    assert!(matches("/crates/**", "crates/core/src/lib.rs"));
    assert!(matches("crates/**", "crates/Cargo.toml"));
    assert!(!matches("/crates/**", "crates"));
    assert!(!matches("/crates/**", "x/crates/a"));
}

#[test]
fn a_bare_double_star_matches_everything() {
    assert!(matches("**", "README.md"));
    assert!(matches("**", "a/b/c.txt"));
}

#[test]
fn paths_are_case_sensitive() {
    assert!(!matches("/Docs/", "docs/a.md"));
    assert!(!matches("*.MD", "a.md"));
}

#[test]
fn a_rule_with_no_owners_matches_and_leaves_its_paths_unowned() {
    // GitHub: `/apps/github` with no owners leaves that subdirectory without an owner, while
    // `/apps/` keeps the rest of the directory.
    let owners = rules("/apps/ @octocat\n/apps/github\n");
    let rule = owners.matching_rule("apps/github/a.rb").unwrap();
    assert_eq!(rule.line, 2);
    assert_eq!(rule.owners, []);
    assert_eq!(
        owners.matching_rule("apps/other/a.rb").unwrap().owners,
        [user("octocat")]
    );
}

#[test]
fn a_later_rule_gives_a_subdirectory_its_own_owner() {
    // GitHub: `/apps/github @doctocat` after `/apps/ @octocat`.
    let owners = rules("/apps/ @octocat\n/apps/github @doctocat\n");
    assert_eq!(
        owners.matching_rule("apps/github/a.rb").unwrap().owners,
        [user("doctocat")]
    );
}

#[test]
fn users_teams_and_emails_are_owners() {
    let owners = rules("* @Octo-Cat @MintSwarm/Core-Team docs@example.com @mintswarm-dev[bot]\n");
    assert_eq!(
        owners.rules()[0].owners,
        [
            user("Octo-Cat"),
            Owner::Team(Team::new("mintswarm", "core-team")),
            Owner::Email("docs@example.com".to_owned()),
            user("mintswarm-dev[bot]"),
        ]
    );
}

#[test]
fn teams_are_stored_in_lower_case() {
    let team = Team::new("MintSwarm", "Core.Team_1");
    assert_eq!(team.org, "mintswarm");
    assert_eq!(team.slug, "core.team_1");
}

#[test]
fn comments_and_blank_lines_are_skipped() {
    // GitHub: `*.js @js-owner #This is an inline comment.`
    let parsed = parse("# owners\n\n   \n*.js @js-owner #This is an inline comment.\n");
    assert_eq!(parsed.errors, []);
    let rule = &parsed.rules.rules()[0];
    assert_eq!(rule.line, 4);
    assert_eq!(rule.pattern, "*.js");
    assert_eq!(rule.owners, [user("js-owner")]);
}

#[test]
fn an_owner_comment_ends_the_owners_even_when_invalid_text_follows() {
    let owners = rules("/a @x # not an owner: !!\n");
    assert_eq!(owners.rules()[0].owners, [user("x")]);
}

#[test]
fn escaped_hash_is_a_literal_hash() {
    let owners = rules("\\#notes.md @a\n");
    assert_eq!(owners.rules()[0].pattern, "\\#notes.md");
    assert_eq!(line(&owners, "#notes.md"), Some(1));
    assert_eq!(line(&owners, "dir/#notes.md"), Some(1));
    assert_eq!(line(&owners, "notes.md"), None);
}

#[test]
fn whitespace_around_a_line_is_ignored() {
    let owners = rules("  /docs/\t@a  \r\n");
    assert_eq!(line(&owners, "docs/a.md"), Some(1));
}

fn error_kind(line_text: &str) -> LineErrorKind {
    let parsed = parse(&format!("* @first\n{line_text}\n/last/ @last\n"));
    assert_eq!(parsed.errors.len(), 1, "one line is left out");
    assert_eq!(parsed.errors[0].line, 2);
    let lines: Vec<usize> = parsed.rules.rules().iter().map(|r| r.line).collect();
    assert_eq!(
        lines,
        [1, 3],
        "the bad line is skipped and later lines kept"
    );
    parsed.errors[0].kind
}

#[test]
fn negation_is_left_out() {
    assert_eq!(error_kind("!/docs/ @a"), LineErrorKind::Negation);
}

#[test]
fn character_classes_are_left_out() {
    assert_eq!(error_kind("/src/[ab].rs @a"), LineErrorKind::CharacterClass);
    assert_eq!(error_kind("/src/a].rs @a"), LineErrorKind::CharacterClass);
}

#[test]
fn other_escapes_are_left_out() {
    assert_eq!(error_kind("/a\\b @a"), LineErrorKind::UnsupportedEscape);
    assert_eq!(error_kind("/a\\*b @a"), LineErrorKind::UnsupportedEscape);
}

#[test]
fn empty_patterns_are_left_out() {
    assert_eq!(error_kind("/ @a"), LineErrorKind::EmptyPattern);
    assert_eq!(error_kind("// @a"), LineErrorKind::EmptyPattern);
}

#[test]
fn invalid_owners_are_left_out() {
    assert_eq!(error_kind("/a octocat"), LineErrorKind::InvalidOwner);
    assert_eq!(error_kind("/a @octo_cat"), LineErrorKind::InvalidOwner);
    assert_eq!(error_kind("/a @org/"), LineErrorKind::InvalidOwner);
    assert_eq!(error_kind("/a @"), LineErrorKind::InvalidOwner);
    assert_eq!(error_kind("/a a@b@c"), LineErrorKind::InvalidOwner);
}

#[test]
fn a_skipped_line_does_not_own_its_paths() {
    let parsed = parse("* @all\n/docs/ octocat\n");
    assert_eq!(parsed.rules.matching_rule("docs/a.md").unwrap().line, 1);
}
