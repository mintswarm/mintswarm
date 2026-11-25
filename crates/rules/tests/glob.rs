//! Glob matching against the table and pinned cases of `check_globs` in
//! `.mintswarm/dogfood/plan-approval/selftest.py`, copied verbatim.

use mintswarm_rules::glob::{Glob, matching};

const PATHS: [&str; 23] = [
    "docs",
    "docs/a.md",
    "docs/x/y.md",
    "x.md",
    "a/x.md",
    "a/b/x.md",
    "a/b",
    "a/c/d/b",
    "a/",
    "a.md",
    "ab.md",
    "abc",
    "a/c",
    "aXb",
    "a.b",
    "[ab].md",
    "a+b(c)",
    "README.md",
    "readme.md",
    ".mintswarm/dogfood/github/push.sh",
    ".mintswarm/dogfood/githubx",
    "crates/core/src/lib.rs",
    "crates/core/x/src/lib.rs",
];

/// Each pattern with exactly the paths it matches; `None` stands for every path.
const MATCHES: [(&str, Option<&[&str]>); 15] = [
    ("**", None),
    ("docs/**", Some(&["docs/a.md", "docs/x/y.md"])),
    ("**/x.md", Some(&["x.md", "a/x.md", "a/b/x.md"])),
    ("a/**/b", Some(&["a/b", "a/c/d/b"])),
    (
        "a/**",
        Some(&["a/x.md", "a/b/x.md", "a/b", "a/c/d/b", "a/", "a/c"]),
    ),
    (
        "*.md",
        Some(&["x.md", "a.md", "ab.md", "[ab].md", "README.md", "readme.md"]),
    ),
    ("docs/*.md", Some(&["docs/a.md"])),
    ("?.md", Some(&["x.md", "a.md"])),
    ("a?c", Some(&["abc"])),
    ("a.b", Some(&["a.b"])),
    ("[ab].md", Some(&["[ab].md"])),
    ("a+b(c)", Some(&["a+b(c)"])),
    ("README.md", Some(&["README.md"])),
    (
        ".mintswarm/dogfood/github/**",
        Some(&[".mintswarm/dogfood/github/push.sh"]),
    ),
    ("crates/*/src/**", Some(&["crates/core/src/lib.rs"])),
];

const PINNED: [(&str, &str, bool); 9] = [
    ("**/x.md", "x.md", true),
    ("**/x.md", "a/b/x.md", true),
    ("*.md", "docs/a.md", false),
    ("?.md", "a.md", true),
    ("a?c", "a/c", false),
    ("a.b", "aXb", false),
    ("[ab].md", "a.md", false),
    ("[ab].md", "[ab].md", true),
    ("docs/**", "docs", false),
];

#[test]
fn every_pattern_and_path_pair_matches_as_floor_py_does() {
    let mut differ = Vec::new();
    for (pattern, want) in MATCHES {
        let glob = Glob::new(pattern);
        for path in PATHS {
            let expected = want.is_none_or(|want| want.contains(&path));
            if glob.matches(path) != expected {
                differ.push((pattern, path));
            }
        }
    }
    assert_eq!(MATCHES.len() * PATHS.len(), 345);
    assert!(differ.is_empty(), "{differ:?}");
}

#[test]
fn pinned_cases_match_as_floor_py_does() {
    let wrong: Vec<_> = PINNED
        .iter()
        .filter(|(pattern, path, want)| Glob::new(pattern).matches(path) != *want)
        .collect();
    assert!(wrong.is_empty(), "{wrong:?}");
}

#[test]
fn a_newline_is_an_ordinary_character() {
    assert!(Glob::new("a*b").matches("a\nb"));
    assert!(Glob::new("a?b").matches("a\nb"));
    assert!(Glob::new("**").matches("x\n/y"));
}

#[test]
fn case_insensitive_matching_folds_by_lowercase() {
    assert!(!Glob::new("README.md").matches("readme.md"));
    assert!(Glob::new("README.md").matches_ci("readme.md"));
    assert!(Glob::new("readme.md").matches_ci("README.md"));
    assert!(Glob::new("docs/ÄRGER/*.md").matches_ci("docs/ärger/x.md"));
    assert!(!Glob::new("docs/ÄRGER/*.md").matches("docs/ärger/x.md"));
    assert!(!Glob::new("*.MD").matches_ci("a/x.md"));
}

#[test]
fn dotted_capital_i_folds_to_i_as_in_python() {
    // U+0130's full lowercase is two characters; its simple lowercase, which `re.IGNORECASE` uses, is `i`.
    assert!(Glob::new("\u{130}.md").matches_ci("i.md"));
    assert!(Glob::new("i.md").matches_ci("\u{130}.md"));
    assert!(Glob::new("I.md").matches_ci("\u{130}.md"));
    assert!(!Glob::new("\u{130}.md").matches("i.md"));
}

#[test]
fn matching_keeps_the_given_order() {
    let globs = ["*.md", "docs/**", "**", "README.md"];
    assert_eq!(matching(&globs, "readme.md", false), vec!["*.md", "**"]);
    assert_eq!(
        matching(&globs, "readme.md", true),
        vec!["*.md", "**", "README.md"]
    );
    assert_eq!(matching(&globs, "docs/a.md", false), vec!["docs/**", "**"]);
}
