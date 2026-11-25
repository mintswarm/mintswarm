//! Probe definitions: their schema, rejections by key path, and the three-source merge.

#![cfg(test)]

use std::time::Duration;

use mintswarm_config::{
    DEFAULT_PROBE_TIMEOUT, DetectMethod, ProbeDefinition, ProbeOrigin, load_probe_definition,
    merge_probe_definitions,
};
use mintswarm_domain::probe::ProbeKind;

const GIT: &str = "\
id: git
kind: tool
detect:
  - command: git --version
    version_regex: 'git version (\\S+)'
";

fn rejected(text: &str) -> mintswarm_config::Error {
    load_probe_definition(text).expect_err("the definition is rejected")
}

#[test]
fn unknown_kind_names_kind() {
    let error = rejected(&GIT.replace("kind: tool", "kind: gadget"));
    assert_eq!(error.path(), "kind", "{error}");
    assert!(error.message().contains("gadget"), "{error}");
}

#[test]
fn missing_kind_and_detect_are_rejected() {
    let error = rejected("id: git\ndetect: [{env: GIT}]\n");
    assert_eq!(error.path(), "kind", "{error}");
    let error = rejected("id: git\nkind: tool\n");
    assert_eq!(error.path(), "detect", "{error}");
    let error = rejected("id: git\nkind: tool\ndetect: []\n");
    assert_eq!(error.path(), "detect", "{error}");
}

#[test]
fn unknown_keys_name_their_path() {
    let error = rejected(&format!("{GIT}requires: '>= 2.45'\n"));
    assert_eq!(error.path(), "requires", "{error}");
    assert_eq!(error.message(), "unknown key");

    let error = rejected(&format!("{GIT}    shell: bash\n"));
    assert_eq!(error.path(), "detect[0].shell", "{error}");
    assert_eq!(error.message(), "unknown key");

    let error = rejected("id: x\nkind: tool\ndetect:\n  - env: X\n    version_regex: '(.*)'\n");
    assert_eq!(error.path(), "detect[0].version_regex", "{error}");
}

#[test]
fn missing_id_names_id() {
    let error = rejected(&GIT.replace("id: git\n", ""));
    assert_eq!(error.path(), "id", "{error}");
    assert_eq!(error.message(), "is missing");
    let error = rejected(&GIT.replace("id: git", "id: ''"));
    assert_eq!(error.path(), "id", "{error}");
}

#[test]
fn duplicate_key_names_its_path() {
    let error = rejected(&format!("{GIT}kind: engine\n"));
    assert_eq!(error.path(), "kind", "{error}");
    assert!(error.message().contains("duplicate key"), "{error}");

    let error = rejected(&format!("{GIT}    command: git version\n"));
    assert_eq!(error.path(), "detect[0].command", "{error}");
    assert!(error.message().contains("duplicate key"), "{error}");
}

#[test]
fn detect_items_hold_one_method() {
    let error = rejected("id: x\nkind: tool\ndetect:\n  - {}\n");
    assert_eq!(error.path(), "detect[0]", "{error}");
    let error = rejected("id: x\nkind: tool\ndetect:\n  - env: X\n    path: /x\n");
    assert_eq!(error.path(), "detect[0].env", "{error}");
    let error = rejected("id: x\nkind: tool\ndetect:\n  - command: x --version\n");
    assert_eq!(error.path(), "detect[0].version_regex", "{error}");
    let error = rejected("id: x\nkind: tool\ndetect:\n  - env: ''\n");
    assert_eq!(error.path(), "detect[0].env", "{error}");
    let error = rejected("id: x\nkind: tool\ndetect:\n  - script\n");
    assert_eq!(error.path(), "detect[0]", "{error}");
}

#[test]
fn every_detect_method_loads() {
    let definition = load_probe_definition(
        "\
id: claude
kind: harness
detect:
  - command: claude --version
    version_regex: '^(\\S+)'
  - path: ~/.local/bin/claude*
  - env: CLAUDE_CODE_PATH
  - script: probes/claude-auth.sh
timeout: 2.5
",
    )
    .expect("loads");
    assert_eq!(definition.id(), "claude");
    assert_eq!(definition.kind(), ProbeKind::Harness);
    assert_eq!(definition.timeout(), Duration::from_millis(2500));
    assert_eq!(
        definition.detect(),
        [
            DetectMethod::Command {
                command: "claude --version".into(),
                version_regex: "^(\\S+)".into(),
            },
            DetectMethod::Path {
                path: "~/.local/bin/claude*".into(),
            },
            DetectMethod::Env {
                env: "CLAUDE_CODE_PATH".into(),
            },
            DetectMethod::Script {
                script: "probes/claude-auth.sh".into(),
            },
        ]
    );
}

#[test]
fn timeout_defaults_and_bounds() {
    let definition = load_probe_definition(GIT).expect("loads");
    assert_eq!(definition.timeout(), DEFAULT_PROBE_TIMEOUT);
    assert_eq!(DEFAULT_PROBE_TIMEOUT, Duration::from_secs(10));
    let definition = load_probe_definition(&format!("{GIT}timeout: 30\n")).expect("loads");
    assert_eq!(definition.timeout(), Duration::from_secs(30));
    for value in ["0", "-1", "'10'", "true", "1e-12"] {
        let error = rejected(&format!("{GIT}timeout: {value}\n"));
        assert_eq!(error.path(), "timeout", "{value}: {error}");
    }
}

#[test]
fn every_kind_loads() {
    for kind in ProbeKind::ALL {
        let text = GIT.replace("kind: tool", &format!("kind: {kind}"));
        assert_eq!(load_probe_definition(&text).expect("loads").kind(), kind);
    }
    let error = rejected("- id: git\n");
    assert_eq!(error.path(), "", "{error}");
}

fn definition(id: &str, kind: &str, env: &str) -> ProbeDefinition {
    load_probe_definition(&format!(
        "id: {id}\nkind: {kind}\ndetect:\n  - env: {env}\n"
    ))
    .expect("loads")
}

fn source(definitions: &[(&str, ProbeDefinition)]) -> Vec<(String, ProbeDefinition)> {
    definitions
        .iter()
        .map(|(name, definition)| ((*name).to_owned(), definition.clone()))
        .collect()
}

#[test]
fn repository_replaces_machine_replaces_builtin() {
    let builtin = source(&[
        ("git.yaml", definition("git", "tool", "BUILTIN")),
        ("bash.yaml", definition("bash", "tool", "BASH")),
    ]);
    let machine = source(&[
        ("git.yaml", definition("git", "tool", "MACHINE")),
        ("xcode.yaml", definition("xcode", "toolchain", "XCODE")),
    ]);
    let repository = source(&[
        ("git.yaml", definition("git", "tool", "REPOSITORY")),
        ("unity.yaml", definition("unity", "engine", "UNITY")),
    ]);
    let merged = merge_probe_definitions(&builtin, &machine, &repository).expect("merges");

    assert_eq!(
        merged.keys().map(String::as_str).collect::<Vec<_>>(),
        ["bash", "git", "unity", "xcode"]
    );
    assert_eq!(
        merged["git"],
        (
            ProbeOrigin::Repository,
            definition("git", "tool", "REPOSITORY")
        )
    );
    assert_eq!(merged["bash"].0, ProbeOrigin::Builtin);
    assert_eq!(merged["xcode"].0, ProbeOrigin::Machine);
    assert_eq!(merged["unity"].0, ProbeOrigin::Repository);

    let merged = merge_probe_definitions(&builtin, &machine, &[]).expect("merges");
    assert_eq!(
        merged["git"],
        (ProbeOrigin::Machine, definition("git", "tool", "MACHINE"))
    );
}

#[test]
fn duplicate_id_within_a_source_is_rejected() {
    let machine = source(&[
        ("git.yaml", definition("git", "tool", "A")),
        ("git-too.yaml", definition("git", "tool", "B")),
    ]);
    let error = merge_probe_definitions(&[], &machine, &[]).expect_err("a duplicate id");
    assert_eq!(error.path(), "id", "{error}");
    for part in ["machine", "git.yaml", "git-too.yaml", "`git`"] {
        assert!(error.message().contains(part), "{part}: {error}");
    }
}
