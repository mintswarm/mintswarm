//! The activity classifier: shell normalisation, matching by kind, and precedence.

#![cfg(test)]

use mintswarm_config::{ActivityRule, load_activity_rules};
use mintswarm_domain::facts::catalogue::{ActivitySource, Harness};
use mintswarm_rules::activity::shell::{Normalised, ShellUnreadable, normalise};
use mintswarm_rules::activity::{Call, Classification, OTHER, Rules};

/// The rules of a rules file holding the rule lines `lines`.
fn rules(lines: &[&str]) -> Vec<ActivityRule> {
    let text: String = std::iter::once("rules:\n".to_owned())
        .chain(lines.iter().map(|line| format!("  - {line}\n")))
        .collect();
    load_activity_rules(&text).unwrap()
}

fn shell(command: &str) -> Call {
    Call::Shell(command.to_owned())
}

fn classified(activity: &str, source: ActivitySource) -> Classification {
    Classification {
        activity: activity.to_owned(),
        source,
    }
}

fn program(command: &str) -> Normalised {
    normalise(command).unwrap_or_else(|error| panic!("{command}: {error}"))
}

fn normalised(program: &str, args: &[&str]) -> Normalised {
    Normalised {
        program: program.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    }
}

#[test]
fn the_source_is_the_catalogue_type() {
    let classification = Rules::new(&[], &[]).classify(Harness::Claude, &Call::Tool("Read".into()));
    let _: ActivitySource = classification.source;
    let _: mintswarm_domain::facts::catalogue::ActivitySource = classification.source;
}

// Step 1: POSIX words.

#[test]
fn step_1_words_honour_quotes_and_escapes() {
    assert_eq!(
        program(r#"grep "a b" 'c d' e\ f "g\"h" 'i\j'"#),
        normalised("grep", &["a b", "c d", "e f", "g\"h", "i\\j"])
    );
    assert_eq!(program("echo ''"), normalised("echo", &[""]));
    assert_eq!(program("ls # a comment"), normalised("ls", &[]));
}

#[test]
fn step_1_a_line_continuation_starts_no_word() {
    assert_eq!(program("cargo \\\n test"), normalised("cargo", &["test"]));
    assert_eq!(
        program("cd x && \\\n git commit"),
        normalised("git", &["commit"])
    );
    assert_eq!(program("ec\\\nho a"), normalised("echo", &["a"]));
}

// Step 2: commands split on operators.

#[test]
fn step_2_commands_split_on_operators() {
    for command in [
        "grep a b | wc",
        "grep a b|wc",
        "grep a b && wc",
        "grep a b||wc",
        "grep a b; wc",
        "grep a b & wc",
        "grep a b\nwc",
    ] {
        assert_eq!(
            program(command),
            normalised("grep", &["a", "b"]),
            "{command:?}"
        );
    }
    assert_eq!(
        program("cargo test 2>&1 | tail"),
        normalised("cargo", &["test"])
    );
}

// Step 3: leading cd commands and assignments.

#[test]
fn step_3_leading_cd_and_assignments_are_dropped() {
    assert_eq!(program("cd /x && ls -l"), normalised("ls", &["-l"]));
    assert_eq!(
        program("cd /x; cd y && FOO=1 BAR=2 ls"),
        normalised("ls", &[])
    );
    assert_eq!(program("cd /x"), normalised("cd", &["/x"]));
    assert_eq!(program("\"FOO=1\" ls"), normalised("FOO=1", &["ls"]));
}

#[test]
fn step_3_a_leading_negation_is_dropped() {
    assert_eq!(program("! git commit"), normalised("git", &["commit"]));
    assert_eq!(
        program("cd x && ! FOO=1 grep a"),
        normalised("grep", &["a"])
    );
    assert_eq!(program("'!' x"), normalised("!", &["x"]));
}

#[test]
fn the_first_literal_example_gives_git_commit() {
    let result = program("cd /x && FOO=1 env git commit -m y");
    assert_eq!(result.program, "git");
    assert_eq!(result.args.first().map(String::as_str), Some("commit"));
}

// Step 4: wrappers.

#[test]
fn step_4_wrappers_are_unwrapped() {
    assert_eq!(program("env -i A=1 git log"), normalised("git", &["log"]));
    assert_eq!(program("env -u HOME git log"), normalised("git", &["log"]));
    assert_eq!(program("command -p ls"), normalised("ls", &[]));
    assert_eq!(
        program("time -p env cargo build"),
        normalised("cargo", &["build"])
    );
    assert_eq!(
        program("/usr/bin/env python3 x.py"),
        normalised("python3", &["x.py"])
    );
    assert_eq!(program("env"), normalised("env", &[]));
    assert_eq!(
        program("command -v git"),
        normalised("command", &["-v", "git"])
    );
    assert_eq!(
        program("command -V git"),
        normalised("command", &["-V", "git"])
    );
    assert_eq!(
        program("env command -pv git"),
        normalised("command", &["-pv", "git"])
    );
    assert_eq!(program("sh -c 'ls -a'"), normalised("ls", &["-a"]));
    assert_eq!(
        program("bash -lc 'cargo test'"),
        normalised("cargo", &["test"])
    );
    assert_eq!(program("zsh -e -c 'make'"), normalised("make", &[]));
    assert_eq!(
        program("bash script.sh"),
        normalised("bash", &["script.sh"])
    );
}

#[test]
fn shells_are_unwrapped_once() {
    assert_eq!(
        program("bash -c \"sh -c 'ls'\""),
        normalised("sh", &["-c", "ls"])
    );
}

#[test]
fn the_second_literal_example_gives_grep() {
    assert_eq!(program("bash -c 'grep a b | wc'").program, "grep");
}

// Step 5: the basename.

#[test]
fn step_5_the_program_is_the_basename() {
    assert_eq!(
        program("/usr/bin/git status"),
        normalised("git", &["status"])
    );
    assert_eq!(
        program("./web/node_modules/.bin/tsc --noEmit").program,
        "tsc"
    );
}

#[test]
fn unreadable_commands_are_refused() {
    for command in [
        "(cd x; ls)",
        "echo $(date)",
        "echo \"$(date)\"",
        "echo `date`",
        "cat <<EOF\nx\nEOF",
        "echo 'a",
        "echo \"a",
        "echo a\\",
        "diff <(ls) x",
        "ls >",
        "FOO=1",
        "",
    ] {
        assert_eq!(normalise(command), Err(ShellUnreadable), "{command:?}");
    }
    assert_eq!(program("echo '$(date)'"), normalised("echo", &["$(date)"]));
}

#[test]
fn unreadable_commands_classify_as_unparsed() {
    let rules = Rules::new(&rules(&["{ command: \"*\", activity: running }"]), &[]);
    for command in [
        "(cd x; ls)",
        "echo $(date)",
        "echo `date`",
        "cat <<EOF\nx\nEOF",
        "echo 'a",
    ] {
        assert_eq!(
            rules.classify(Harness::Claude, &shell(command)),
            classified(OTHER, ActivitySource::Unparsed),
            "{command:?}"
        );
    }
}

#[test]
fn an_unmatched_command_classifies_as_none() {
    let rules = Rules::new(&rules(&["{ command: grep, activity: searching }"]), &[]);
    assert_eq!(
        rules.classify(Harness::Claude, &shell("frobnicate --all")),
        classified(OTHER, ActivitySource::None)
    );
    assert_eq!(
        rules.classify(Harness::Claude, &Call::Tool("Read".into())),
        classified(OTHER, ActivitySource::None)
    );
}

#[test]
fn command_args_match_the_leading_arguments() {
    let rules = Rules::new(
        &rules(&[
            "{ command: git, activity: reading }",
            "{ command: git, args: [commit], activity: committing }",
        ]),
        &[],
    );
    let classify = |command| rules.classify(Harness::Claude, &shell(command)).activity;
    assert_eq!(classify("git commit -m x"), "committing");
    assert_eq!(classify("git -C dir commit"), "reading");
    assert_eq!(classify("git"), "reading");
}

// Precedence: one test per level, the winner worse on every lower level.

#[test]
fn level_1_more_argument_patterns_win() {
    let defaults = rules(&[
        "{ command: ls, activity: reading }",
        "{ command: \"g?t\", args: [\"c*\"], activity: committing }",
    ]);
    let project = rules(&["{ command: git, activity: reading }"]);
    assert_eq!(
        Rules::new(&defaults, &project).classify(Harness::Claude, &shell("git commit")),
        classified("committing", ActivitySource::Default)
    );
}

#[test]
fn level_2_more_exact_patterns_win() {
    let defaults = rules(&[
        "{ command: ls, activity: reading }",
        "{ command: git, args: [commit], activity: committing }",
    ]);
    let project = rules(&["{ command: git, args: [\"c*\"], activity: reading }"]);
    assert_eq!(
        Rules::new(&defaults, &project).classify(Harness::Claude, &shell("git commit")),
        classified("committing", ActivitySource::Default)
    );
}

#[test]
fn level_3_project_beats_default() {
    let defaults = rules(&["{ command: git, args: [commit], activity: committing }"]);
    let project = rules(&[
        "{ command: ls, activity: reading }",
        "{ command: git, args: [commit], activity: shipping }",
    ]);
    assert_eq!(
        Rules::new(&defaults, &project).classify(Harness::Claude, &shell("git commit")),
        classified("shipping", ActivitySource::Project)
    );
}

#[test]
fn level_4_the_earlier_rule_wins() {
    let project = rules(&[
        "{ tool: Read, activity: reading }",
        "{ tool: Read, activity: searching }",
    ]);
    assert_eq!(
        Rules::new(&[], &project).classify(Harness::Claude, &Call::Tool("Read".into())),
        classified("reading", ActivitySource::Project)
    );
}

// Calls by kind.

#[test]
fn claude_names_map_to_calls() {
    assert_eq!(
        Call::from_harness(Harness::Claude, "Bash", Some("ls")),
        shell("ls")
    );
    assert_eq!(
        Call::from_harness(Harness::Claude, "Read", None),
        Call::Tool("Read".into())
    );
    assert_eq!(
        Call::from_harness(Harness::Claude, "mcp__claude_ai_Google_Drive__search", None),
        Call::Mcp {
            server: "claude_ai_Google_Drive".into(),
            tool: "search".into()
        }
    );
    assert_eq!(
        Call::from_harness(Harness::Claude, "mcp__lonely", None),
        Call::Tool("mcp__lonely".into())
    );
    assert_eq!(
        Call::from_harness(Harness::Codex, "shell", Some("ls")),
        shell("ls")
    );
    assert_eq!(
        Call::from_harness(Harness::Codex, "Bash", Some("ls")),
        Call::Tool("Bash".into())
    );
}

#[test]
fn mcp_rules_match_server_and_tool_with_globs() {
    let rules = Rules::new(
        &rules(&[
            "{ mcp: { server: github, tool: \"*\" }, activity: fetching }",
            "{ mcp: { server: \"claude_ai_*\", tool: \"search*\" }, activity: searching }",
        ]),
        &[],
    );
    let classify = |name| {
        rules.classify(
            Harness::Claude,
            &Call::from_harness(Harness::Claude, name, None),
        )
    };
    assert_eq!(
        classify("mcp__github__get_issue"),
        classified("fetching", ActivitySource::Default)
    );
    assert_eq!(
        classify("mcp__claude_ai_Google_Drive__search_files"),
        classified("searching", ActivitySource::Default)
    );
    assert_eq!(
        classify("mcp__claude_ai_Google_Drive__read_file"),
        classified(OTHER, ActivitySource::None)
    );
    assert_eq!(
        classify("mcp__gitlab__get_issue"),
        classified(OTHER, ActivitySource::None)
    );
}

#[test]
fn a_tool_glob_does_not_classify_an_mcp_call() {
    let rules = Rules::new(&rules(&["{ tool: \"*\", activity: running }"]), &[]);
    let call = Call::from_harness(Harness::Claude, "mcp__github__get_issue", None);
    assert_eq!(
        rules.classify(Harness::Claude, &call),
        classified(OTHER, ActivitySource::None)
    );
}

#[test]
fn a_shell_call_is_matched_only_by_command_rules() {
    let rules = Rules::new(&rules(&["{ tool: Bash, activity: running }"]), &[]);
    assert_eq!(
        rules.classify(Harness::Claude, &shell("ls")),
        classified(OTHER, ActivitySource::None)
    );
}

#[test]
fn a_harness_filter_skips_other_harnesses() {
    let rules = Rules::new(
        &rules(&["{ tool: \"Web*\", harness: [claude], activity: fetching }"]),
        &[],
    );
    let call = Call::Tool("WebFetch".into());
    assert_eq!(
        rules.classify(Harness::Claude, &call),
        classified("fetching", ActivitySource::Default)
    );
    assert_eq!(
        rules.classify(Harness::Codex, &call),
        classified(OTHER, ActivitySource::None)
    );
}

#[test]
fn argument_globs_follow_the_documented_syntax() {
    let rules = Rules::new(
        &rules(&[
            "{ command: python3, args: [\"*.py\"], activity: running }",
            "{ command: python3, args: [\"**.py\"], activity: testing }",
        ]),
        &[],
    );
    let classify = |command| rules.classify(Harness::Claude, &shell(command)).activity;
    // the rules tie up to file order, so the earlier wins where both match
    assert_eq!(classify("python3 x.py"), "running");
    assert_eq!(classify("python3 scripts/x.py"), "testing");
    assert_eq!(classify("python3 X.PY"), OTHER);
}
