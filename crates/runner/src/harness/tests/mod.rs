//! Tests of stage rendering: parity with `render-agents.py` on this repository's stages, one
//! case per rule, the agent precedence, and reads from the trusted store only.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use mintswarm_config::{RepoConfig, load_repo_config, parse_yaml};
use mintswarm_domain::ids::RevId;
use mintswarm_domain::vcs::{RefOrId, Remote, Vcs};
use mintswarm_rules::binding::digest;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::render::{
    Agent, CONVENTIONS, Capability, Frontmatter, RenderError, STAGES_DIR, Tier, choose_agent,
    parse, render, tier_model,
};
use crate::exec::git::tests::{fixture_command, git_probe, run_fixture, text};
use crate::exec::git::{GitStore, store_path};

/// The repository this crate is in.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `render-agents.py`'s Claude tools for a capability.
fn tools(capability: Capability) -> &'static [&'static str] {
    match capability {
        Capability::Read => &["Read"],
        Capability::Search => &["Grep", "Glob"],
        Capability::Shell => &["Bash"],
        Capability::Edit => &["Edit", "Write"],
    }
}

/// `render-agents.py`'s `TOOL_ORDER`.
const TOOL_ORDER: [&str; 6] = ["Read", "Grep", "Glob", "Bash", "Edit", "Write"];

/// `render-agents.py`'s `quote()`.
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// What `render-agents.py`'s `render()` writes for a parsed stage.
fn claude_agent(frontmatter: &Frontmatter, body: &str) -> String {
    let wanted: Vec<&str> = frontmatter
        .capabilities
        .iter()
        .flat_map(|capability| tools(*capability).iter().copied())
        .collect();
    let ordered: Vec<&str> = TOOL_ORDER
        .into_iter()
        .filter(|tool| wanted.contains(tool))
        .collect();
    let name = &frontmatter.name;
    format!(
        "---\n\
         # Generated from .mintswarm/stages/{name}.md by .mintswarm/dogfood/render-agents.py; do not edit.\n\
         name: msw-{name}\n\
         description: {}\n\
         tools: {}\n\
         model: {}\n\
         ---\n\
         {body}",
        quote(&frontmatter.description),
        ordered.join(", "),
        tier_model("claude", frontmatter.tier).unwrap(),
    )
}

#[test]
fn every_stage_matches_the_python_rendering() {
    let root = root();
    let mut stages = Vec::new();
    for entry in std::fs::read_dir(root.join(STAGES_DIR)).unwrap() {
        let path = entry.unwrap().path();
        let file = path.file_name().unwrap().to_str().unwrap().to_owned();
        if file == "README.md" || !file.ends_with(".md") {
            continue;
        }
        let stage = file.strip_suffix(".md").unwrap().to_owned();
        let bytes = std::fs::read(&path).unwrap();
        let (frontmatter, body) = parse(&stage, &bytes).unwrap();
        assert_eq!(frontmatter.name, stage);
        let agent = std::fs::read(root.join(format!(".claude/agents/msw-{stage}.md"))).unwrap();
        assert_eq!(
            String::from_utf8(agent).unwrap(),
            claude_agent(&frontmatter, &body),
            "msw-{stage}.md differs from the parsed stage"
        );
        // The body is the source's bytes after the closing `---` line.
        assert!(bytes.ends_with(body.as_bytes()));
        stages.push(stage);
    }
    stages.sort();
    let mut agents: Vec<String> = std::fs::read_dir(root.join(".claude/agents"))
        .unwrap()
        .filter_map(|entry| {
            let file = entry.unwrap().file_name().into_string().unwrap();
            file.strip_prefix("msw-")
                .and_then(|rest| rest.strip_suffix(".md"))
                .map(str::to_owned)
        })
        .collect();
    agents.sort();
    assert!(!stages.is_empty());
    assert_eq!(stages, agents);
}

/// A definition of `plan` with `frontmatter` lines and a body.
fn definition(frontmatter: &str) -> String {
    format!("---\n{frontmatter}---\nbody\n")
}

const VALID: &str = "name: plan\ndescription: d\ncapabilities: [read, shell]\ntier: strong\n";

#[test]
fn a_valid_definition_parses() {
    let (frontmatter, body) = parse("plan", definition(VALID).as_bytes()).unwrap();
    assert_eq!(
        frontmatter,
        Frontmatter {
            name: "plan".to_owned(),
            description: "d".to_owned(),
            capabilities: vec![Capability::Read, Capability::Shell],
            tier: Tier::Strong,
        }
    );
    assert_eq!(body, "body\n");
}

#[test]
fn crlf_frontmatter_keeps_the_body_crs() {
    let source = "---\r\nname: plan\r\ndescription: d\r\ncapabilities: [read]\r\n\
                  tier: light\r\n---\r\nline one\r\nline two\r\n";
    let (frontmatter, body) = parse("plan", source.as_bytes()).unwrap();
    assert_eq!(frontmatter.tier, Tier::Light);
    assert_eq!(body, "line one\r\nline two\r\n");
}

#[test]
fn values_are_not_trimmed() {
    let source = definition("name: plan\ndescription: d\ncapabilities: [read]\ntier: strong \n");
    assert!(matches!(
        parse("plan", source.as_bytes()),
        Err(RenderError::UnknownTier(_))
    ));
}

#[test]
fn empty_capability_items_are_dropped_and_duplicates_kept() {
    let source = definition("name: plan\ndescription: d\ncapabilities: [read, ]\ntier: strong\n");
    let (frontmatter, _) = parse("plan", source.as_bytes()).unwrap();
    assert_eq!(frontmatter.capabilities, vec![Capability::Read]);
    let source =
        definition("name: plan\ndescription: d\ncapabilities: [read,read]\ntier: strong\n");
    let (frontmatter, _) = parse("plan", source.as_bytes()).unwrap();
    assert_eq!(
        frontmatter.capabilities,
        vec![Capability::Read, Capability::Read]
    );
}

#[test]
fn a_closing_line_at_the_end_gives_an_empty_body() {
    let source = format!("---\n{VALID}---");
    let (_, body) = parse("plan", source.as_bytes()).unwrap();
    assert_eq!(body, "");
}

#[test]
fn comments_blank_lines_and_python_white_space_are_skipped() {
    // U+001C–U+001F are white space to Python's `str.isspace`, not to Rust's.
    let source = definition(&format!(
        "# a comment\n\n   \n\u{1f}\n\u{1c}# indented comment\n{VALID}"
    ));
    let (frontmatter, _) = parse("plan", source.as_bytes()).unwrap();
    assert_eq!(frontmatter.name, "plan");
    // Capability items are stripped of the same white space.
    let source =
        definition("name: plan\ndescription: d\ncapabilities: [\u{1e}read\u{1d}]\ntier: strong\n");
    let (frontmatter, _) = parse("plan", source.as_bytes()).unwrap();
    assert_eq!(frontmatter.capabilities, vec![Capability::Read]);
}

#[test]
fn a_missing_key_is_rejected_for_each_key() {
    for key in ["name", "description", "capabilities", "tier"] {
        let lines: String = VALID
            .lines()
            .filter(|line| !line.starts_with(&format!("{key}: ")))
            .map(|line| format!("{line}\n"))
            .collect();
        match parse("plan", definition(&lines).as_bytes()) {
            Err(RenderError::MissingKeys(missing)) => assert_eq!(missing, vec![key]),
            other => panic!("{key}: {other:?}"),
        }
    }
}

#[test]
fn empty_capabilities_are_rejected() {
    for list in ["[]", "[ , ]"] {
        let source = definition(&format!(
            "name: plan\ndescription: d\ncapabilities: {list}\ntier: strong\n"
        ));
        assert!(
            matches!(
                parse("plan", source.as_bytes()),
                Err(RenderError::CapabilitiesEmpty)
            ),
            "{list}"
        );
    }
}

#[test]
fn a_name_other_than_the_file_name_is_rejected() {
    assert!(matches!(
        parse("review", definition(VALID).as_bytes()),
        Err(RenderError::NameMismatch { .. })
    ));
}

/// Whether an error is the one a case expects.
type Expected = fn(&RenderError) -> bool;

#[test]
fn every_other_rule_is_rejected() {
    let cases: Vec<(Vec<u8>, Expected)> = vec![
        (b"---\n\xff\n---\n".to_vec(), |e| {
            matches!(e, RenderError::NotUtf8)
        }),
        (format!("--\n{VALID}---\n").into_bytes(), |e| {
            matches!(e, RenderError::NoOpening)
        }),
        (definition("name:plan\n").into_bytes(), |e| {
            matches!(e, RenderError::NotKeyValue { line: 2 })
        }),
        (
            definition(&format!("{VALID}model: opus\n")).into_bytes(),
            |e| matches!(e, RenderError::UnknownKey { line: 6, key } if key == "model"),
        ),
        (
            definition(&format!("{VALID}tier: light\n")).into_bytes(),
            |e| matches!(e, RenderError::DuplicateKey { line: 6, key } if key == "tier"),
        ),
        (format!("---\n{VALID}").into_bytes(), |e| {
            matches!(e, RenderError::NoClosing)
        }),
        (
            definition(
                "name: plan\ndescription: a\u{7}b\u{2028}\tc\ncapabilities: [read]\ntier: strong\n",
            )
            .into_bytes(),
            |e| matches!(e, RenderError::DescriptionCharacters(chars) if chars == &['\u{7}', '\u{2028}']),
        ),
        (
            definition("name: plan\ndescription: d\ncapabilities: read\ntier: strong\n")
                .into_bytes(),
            |e| matches!(e, RenderError::CapabilitiesNotList),
        ),
        (
            definition("name: plan\ndescription: d\ncapabilities: [read, fly]\ntier: strong\n")
                .into_bytes(),
            |e| matches!(e, RenderError::UnknownCapabilities(names) if names == &["fly"]),
        ),
        (
            definition("name: plan\ndescription: d\ncapabilities: [read]\ntier: huge\n")
                .into_bytes(),
            |e| matches!(e, RenderError::UnknownTier(tier) if tier == "huge"),
        ),
    ];
    for (source, expected) in cases {
        let error = parse("plan", &source).unwrap_err();
        assert!(expected(&error), "{error:?}");
    }
}

#[test]
fn rules_are_checked_in_python_order() {
    // A missing key is reported before a name mismatch, which is reported before the rest.
    let source = definition("name: other\ndescription: \u{0}\ncapabilities: x\n");
    assert!(matches!(
        parse("plan", source.as_bytes()),
        Err(RenderError::MissingKeys(_))
    ));
    let source = definition("name: other\ndescription: \u{0}\ncapabilities: x\ntier: x\n");
    assert!(matches!(
        parse("plan", source.as_bytes()),
        Err(RenderError::NameMismatch { .. })
    ));
    let source = definition("name: plan\ndescription: \u{0}\ncapabilities: x\ntier: x\n");
    assert!(matches!(
        parse("plan", source.as_bytes()),
        Err(RenderError::DescriptionCharacters(_))
    ));
}

/// This repository's config.
fn repository_config() -> RepoConfig {
    let text = std::fs::read_to_string(root().join(".mintswarm/config.yaml")).unwrap();
    load_repo_config(&text).unwrap()
}

/// This repository's config with `agents` replaced.
fn config_with_agents(agents: Value) -> RepoConfig {
    let text = std::fs::read_to_string(root().join(".mintswarm/config.yaml")).unwrap();
    let mut value = parse_yaml(&text).unwrap();
    value["agents"] = agents;
    RepoConfig::from_value(value).unwrap()
}

fn agent(harness: &str, model: &str, effort: Option<&str>) -> Agent {
    Agent {
        harness: harness.to_owned(),
        model: model.to_owned(),
        effort: effort.map(str::to_owned),
    }
}

#[test]
fn this_repository_chooses_per_key() {
    let config = repository_config();
    assert_eq!(
        choose_agent(&config, "assess", Tier::Strong).unwrap(),
        agent("claude", "sonnet", Some("high"))
    );
    assert_eq!(
        choose_agent(&config, "gates", Tier::Light).unwrap(),
        agent("claude", "sonnet", Some("low"))
    );
    assert_eq!(
        choose_agent(&config, "plan", Tier::Light).unwrap(),
        agent("claude", "opus", Some("high"))
    );
}

#[test]
fn the_tier_gives_the_model_when_none_is_configured() {
    let config = config_with_agents(json!({
        "default": {"harness": "claude"},
        "stages": {"gates": {"model": "custom"}},
    }));
    for (tier, model) in [
        (Tier::Strong, "opus"),
        (Tier::Standard, "sonnet"),
        (Tier::Light, "haiku"),
    ] {
        assert_eq!(
            choose_agent(&config, "plan", tier).unwrap(),
            agent("claude", model, None)
        );
    }
    assert_eq!(
        choose_agent(&config, "gates", Tier::Light).unwrap(),
        agent("claude", "custom", None)
    );
}

#[test]
fn the_stage_harness_wins_over_the_default() {
    let config = config_with_agents(json!({
        "default": {"harness": "claude", "model": "opus"},
        "stages": {"review": {"harness": "codex", "effort": "medium"}},
    }));
    assert_eq!(
        choose_agent(&config, "review", Tier::Strong).unwrap(),
        agent("codex", "opus", Some("medium"))
    );
}

#[test]
fn a_missing_harness_or_model_is_rejected() {
    let config = config_with_agents(json!({"default": {"model": "opus"}}));
    assert!(matches!(
        choose_agent(&config, "plan", Tier::Strong),
        Err(RenderError::NoHarness { .. })
    ));
    let config = config_with_agents(json!({"stages": {"plan": {"harness": "codex"}}}));
    assert!(matches!(
        choose_agent(&config, "plan", Tier::Strong),
        Err(RenderError::NoModel { harness }) if harness == "codex"
    ));
}

/// A repository with a work tree, and a store to fetch its commits into.
struct Repo {
    /// Keeps the temporary directory alive.
    _dir: TempDir,
    work: PathBuf,
    store: GitStore,
}

impl Repo {
    fn new() -> Repo {
        let dir = TempDir::new().unwrap();
        let work = dir.path().join("work");
        run_fixture(
            fixture_command(None),
            &[
                OsStr::new("init"),
                OsStr::new("--quiet"),
                OsStr::new("--initial-branch=main"),
                work.as_os_str(),
            ],
            b"",
        );
        let path = store_path(&dir.path().join("state"), "repo").unwrap();
        let mut store = GitStore::create(&path, &git_probe(Some("2.45.0"))).unwrap();
        store.allow_file_remote();
        Repo {
            _dir: dir,
            work,
            store,
        }
    }

    /// Git in the work tree.
    fn git(&self, args: &[&str]) -> Vec<u8> {
        let mut all = vec!["-C", self.work.to_str().unwrap()];
        all.extend_from_slice(args);
        run_fixture(fixture_command(None), &all, b"")
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.work.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// Commit `files` and fetch the commit into the store.
    fn commit(&self, files: &[(&str, &[u8])]) -> RevId {
        for (path, bytes) in files {
            self.write(path, bytes);
            self.git(&["add", "--", path]);
        }
        self.git(&["commit", "--quiet", "--allow-empty", "-m", "commit"]);
        let id = RevId(text(&self.git(&["rev-parse", "HEAD"])));
        let remote = Remote {
            name: "origin".to_owned(),
            url: format!("file://{}", self.work.display()),
        };
        self.store
            .fetch(&remote, &[RefOrId::Ref("refs/heads/main".to_owned())], None)
            .unwrap();
        id
    }
}

const STAGE_FILE: &str = ".mintswarm/stages/plan.md";

#[test]
fn a_worktree_change_has_no_effect() {
    let repo = Repo::new();
    let committed = definition(VALID);
    let commit = repo.commit(&[
        (STAGE_FILE, committed.as_bytes()),
        (CONVENTIONS, b"conventions\n"),
    ]);
    let changed =
        definition("name: plan\ndescription: changed\ncapabilities: [edit]\ntier: light\n")
            .replace("body", "injected");
    repo.write(STAGE_FILE, changed.as_bytes());
    assert_eq!(
        std::fs::read(repo.work.join(STAGE_FILE)).unwrap(),
        changed.as_bytes()
    );

    let rendered = render(&repo.store, &commit, "plan", &repository_config()).unwrap();
    assert_eq!(rendered.body, "body\n");
    assert_eq!(rendered.frontmatter.description, "d");
    assert_eq!(rendered.instructions, commit);
    assert_eq!(rendered.stage_file.path, STAGE_FILE);
    assert_eq!(rendered.stage_file.sha256, digest(committed.as_bytes()));
    assert_eq!(rendered.conventions.path, CONVENTIONS);
    assert_eq!(rendered.conventions.sha256, digest(b"conventions\n"));
    assert_eq!(rendered.agent, agent("claude", "opus", Some("high")));
}

#[test]
fn render_rejects_bad_names_and_missing_files() {
    let repo = Repo::new();
    let config = repository_config();
    let only_stage = repo.commit(&[(STAGE_FILE, definition(VALID).as_bytes())]);
    assert!(matches!(
        render(&repo.store, &only_stage, "plan", &config),
        Err(RenderError::MissingConventions { .. })
    ));
    assert!(matches!(
        render(&repo.store, &only_stage, "review", &config),
        Err(RenderError::MissingStage { .. })
    ));
    for name in [
        "", "README", "../plan", "plan/x", "1plan", "-plan", "plan.md",
    ] {
        assert!(
            matches!(
                render(&repo.store, &only_stage, name, &config),
                Err(RenderError::InvalidName(_))
            ),
            "{name:?}"
        );
    }
    let absent = RevId("0123456789abcdef0123456789abcdef01234567".to_owned());
    assert!(matches!(
        render(&repo.store, &absent, "plan", &config),
        Err(RenderError::Store { .. })
    ));
}
