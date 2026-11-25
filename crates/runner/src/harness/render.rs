//! Stage definitions rendered from the trusted store at a job's base commit, design §7.
//!
//! A stage's instructions are `.mintswarm/stages/<stage>.md`, read with [`Vcs::read_blob`] at a
//! full commit id and never from a worktree, so nothing a stage writes reaches the next one's
//! instructions. [`parse`] reads its frontmatter by `.mintswarm/stages/README.md`'s subset, as
//! `render-agents.py` does, and keeps the body byte for byte. [`choose_agent`] picks harness,
//! model and effort per key from `agents.stages.<stage>`, then `agents.default`, and the model
//! last from the stage's tier. [`render`] binds the stage file and `.mintswarm/stages.md` to
//! the sha256 of the bytes read.

use mintswarm_config::RepoConfig;
use mintswarm_domain::ids::RevId;
use mintswarm_domain::records::Bound;
use mintswarm_domain::vcs::{RepoPath, Vcs};
use mintswarm_rules::binding::bind;
use serde_json::{Map, Value};

/// The directory of the stage definitions.
pub const STAGES_DIR: &str = ".mintswarm/stages";

/// The rules every stage follows, which the definitions refer to.
pub const CONVENTIONS: &str = ".mintswarm/stages.md";

/// The frontmatter keys, all required, in the order a missing one is reported.
pub const KEYS: [&str; 4] = ["name", "description", "capabilities", "tier"];

/// What a stage needs from its harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Read files in the worktree.
    Read,
    /// Search file names and contents.
    Search,
    /// Run shell commands.
    Shell,
    /// Edit and create files in the repository.
    Edit,
}

impl Capability {
    /// Every capability.
    pub const ALL: [Capability; 4] = [
        Capability::Read,
        Capability::Search,
        Capability::Shell,
        Capability::Edit,
    ];

    /// The capability a definition names `name`.
    pub fn from_name(name: &str) -> Option<Capability> {
        Capability::ALL
            .into_iter()
            .find(|capability| capability.name() == name)
    }

    /// Its name in a definition.
    pub fn name(self) -> &'static str {
        match self {
            Capability::Read => "read",
            Capability::Search => "search",
            Capability::Shell => "shell",
            Capability::Edit => "edit",
        }
    }
}

/// A stage's default model class, used when the config names no model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tier {
    /// The most capable model.
    Strong,
    /// A balanced model.
    Standard,
    /// A small, fast model.
    Light,
}

impl Tier {
    /// Every tier.
    pub const ALL: [Tier; 3] = [Tier::Strong, Tier::Standard, Tier::Light];

    /// The tier a definition names `name`.
    pub fn from_name(name: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|tier| tier.name() == name)
    }

    /// Its name in a definition.
    pub fn name(self) -> &'static str {
        match self {
            Tier::Strong => "strong",
            Tier::Standard => "standard",
            Tier::Light => "light",
        }
    }
}

/// The model a tier gives on `harness`, for the harnesses that have a tier table.
pub fn tier_model(harness: &str, tier: Tier) -> Option<&'static str> {
    match harness {
        "claude" => Some(match tier {
            Tier::Strong => "opus",
            Tier::Standard => "sonnet",
            Tier::Light => "haiku",
        }),
        _ => None,
    }
}

/// A stage definition's frontmatter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frontmatter {
    /// The stage's name, equal to the file name without `.md`.
    pub name: String,
    /// The description, as written.
    pub description: String,
    /// The capabilities in source order, duplicates kept.
    pub capabilities: Vec<Capability>,
    /// The default model class.
    pub tier: Tier,
}

/// The harness, model and effort a stage runs with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    /// The harness, such as `claude`.
    pub harness: String,
    /// The model, configured or the tier's.
    pub model: String,
    /// The effort, when configured.
    pub effort: Option<String>,
}

/// A stage rendered at a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedStage {
    /// The definition's frontmatter.
    pub frontmatter: Frontmatter,
    /// The instructions: everything after the closing `---` line, byte for byte.
    pub body: String,
    /// The agent the stage runs with.
    pub agent: Agent,
    /// The commit both files were read at.
    pub instructions: RevId,
    /// The stage file, bound to the sha256 of its bytes.
    pub stage_file: Bound,
    /// `.mintswarm/stages.md`, bound to the sha256 of its bytes.
    pub conventions: Bound,
}

/// Why a stage could not be rendered; each variant is one rule.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The definition is not UTF-8.
    #[error("the stage definition is not UTF-8")]
    NotUtf8,
    /// The first line is not `---`.
    #[error("the stage definition must start with a `---` line")]
    NoOpening,
    /// A frontmatter line is not `key: value`.
    #[error("line {line}: expected `key: value`")]
    NotKeyValue {
        /// The line, counted from 1.
        line: usize,
    },
    /// A frontmatter key is not one of [`KEYS`].
    #[error("line {line}: unknown key {key:?}")]
    UnknownKey {
        /// The line, counted from 1.
        line: usize,
        /// The key.
        key: String,
    },
    /// A frontmatter key is given twice.
    #[error("line {line}: duplicate key {key:?}")]
    DuplicateKey {
        /// The line, counted from 1.
        line: usize,
        /// The key.
        key: String,
    },
    /// No `---` line closes the frontmatter.
    #[error("the frontmatter has no closing `---` line")]
    NoClosing,
    /// Required keys are missing.
    #[error("missing keys: {}", .0.join(", "))]
    MissingKeys(Vec<&'static str>),
    /// `name` is not the file name without `.md`.
    #[error("name {name:?} does not match the file name {stage:?}")]
    NameMismatch {
        /// The `name` given.
        name: String,
        /// The file name without `.md`.
        stage: String,
    },
    /// `description` holds characters that are not allowed.
    #[error("the description contains characters that are not allowed: {}", code_points(.0))]
    DescriptionCharacters(Vec<char>),
    /// `capabilities` is not a `[ ]` list.
    #[error("capabilities must be a [ ] list")]
    CapabilitiesNotList,
    /// `capabilities` is an empty list.
    #[error("capabilities must not be empty")]
    CapabilitiesEmpty,
    /// `capabilities` names unknown capabilities.
    #[error("unknown capabilities: {}", .0.join(", "))]
    UnknownCapabilities(Vec<String>),
    /// `tier` is not a known tier.
    #[error("unknown tier {0:?}")]
    UnknownTier(String),
    /// Neither `agents.stages.<stage>` nor `agents.default` names a harness.
    #[error("no harness is configured for the stage {stage:?}")]
    NoHarness {
        /// The stage.
        stage: String,
    },
    /// No model is configured and the harness has no tier table.
    #[error("no model is configured and the harness {harness:?} has no tier table")]
    NoModel {
        /// The harness.
        harness: String,
    },
    /// The stage name is not a lowercase stem.
    #[error(
        "the stage name {0:?} is not ASCII lowercase letters, digits and `-` starting with a letter"
    )]
    InvalidName(String),
    /// The stage file is not a file at the commit.
    #[error("{path} is not a file at the commit")]
    MissingStage {
        /// The stage file's path.
        path: String,
    },
    /// `.mintswarm/stages.md` is not a file at the commit.
    #[error("{path} is not a file at the commit")]
    MissingConventions {
        /// The conventions file's path.
        path: String,
    },
    /// The trusted store could not be read.
    #[error("{path} could not be read from the trusted store")]
    Store {
        /// The path being read.
        path: String,
        /// What the store reported.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// `U+XXXX` for each character, comma-separated.
fn code_points(chars: &[char]) -> String {
    chars
        .iter()
        .map(|c| format!("U+{:04X}", u32::from(*c)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Python's `str.isspace`: Unicode white space and the separators U+001C–U+001F.
fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python's `str.strip()` with no arguments.
fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// Whether `c` may not appear in a description.
fn disallowed(c: char) -> bool {
    c != '\t' && (c.is_control() || matches!(c, '\u{fffe}' | '\u{ffff}' | '\u{2028}' | '\u{2029}'))
}

/// A frontmatter value: text, or the `capabilities` list.
enum Field {
    Text(String),
    List(Vec<String>),
}

/// Parse the definition of `stage` from its bytes; the frontmatter and the body.
///
/// The rules are `render-agents.py` `parse()`'s, checked in its order: the input splits on
/// LF only, one trailing CR is removed from each frontmatter line, values are not trimmed, and
/// only `capabilities` is read as a `[ ]` list.
pub fn parse(stage: &str, bytes: &[u8]) -> Result<(Frontmatter, String), RenderError> {
    let text = std::str::from_utf8(bytes).map_err(|_| RenderError::NotUtf8)?;
    let pieces: Vec<&str> = text.split('\n').collect();
    let first = pieces[0];
    if first.strip_suffix('\r').unwrap_or(first) != "---" {
        return Err(RenderError::NoOpening);
    }
    let mut fields: [Option<Field>; 4] = [None, None, None, None];
    let mut body = None;
    for (index, piece) in pieces.iter().enumerate().skip(1) {
        let line = index + 1;
        let stripped = piece.strip_suffix('\r').unwrap_or(piece);
        if stripped == "---" {
            body = Some(pieces[index + 1..].join("\n"));
            break;
        }
        if strip(stripped).is_empty() || stripped.trim_start_matches(is_space).starts_with('#') {
            continue;
        }
        let Some((key, value)) = stripped.split_once(": ") else {
            return Err(RenderError::NotKeyValue { line });
        };
        let Some(slot) = KEYS.iter().position(|known| *known == key) else {
            return Err(RenderError::UnknownKey {
                line,
                key: key.to_owned(),
            });
        };
        if fields[slot].is_some() {
            return Err(RenderError::DuplicateKey {
                line,
                key: key.to_owned(),
            });
        }
        fields[slot] = Some(
            if KEYS[slot] == "capabilities" && value.starts_with('[') && value.ends_with(']') {
                let inner = value.get(1..value.len() - 1).unwrap_or("");
                Field::List(
                    inner
                        .split(',')
                        .map(strip)
                        .filter(|item| !item.is_empty())
                        .map(str::to_owned)
                        .collect(),
                )
            } else {
                Field::Text(value.to_owned())
            },
        );
    }
    let body = body.ok_or(RenderError::NoClosing)?;

    let missing: Vec<&'static str> = KEYS
        .iter()
        .zip(&fields)
        .filter(|(_, field)| field.is_none())
        .map(|(key, _)| *key)
        .collect();
    if !missing.is_empty() {
        return Err(RenderError::MissingKeys(missing));
    }
    let [
        Some(name),
        Some(description),
        Some(capabilities),
        Some(tier),
    ] = fields
    else {
        unreachable!("every key is present");
    };
    // Only `capabilities` is ever read as a list.
    let text = |field: Field| match field {
        Field::Text(text) => text,
        Field::List(_) => unreachable!("only capabilities is a list"),
    };
    let (name, description, tier) = (text(name), text(description), text(tier));
    if name != stage {
        return Err(RenderError::NameMismatch {
            name,
            stage: stage.to_owned(),
        });
    }
    let control: Vec<char> = description.chars().filter(|c| disallowed(*c)).collect();
    if !control.is_empty() {
        return Err(RenderError::DescriptionCharacters(control));
    }
    let Field::List(capabilities) = capabilities else {
        return Err(RenderError::CapabilitiesNotList);
    };
    if capabilities.is_empty() {
        return Err(RenderError::CapabilitiesEmpty);
    }
    let unknown: Vec<String> = capabilities
        .iter()
        .filter(|item| Capability::from_name(item).is_none())
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(RenderError::UnknownCapabilities(unknown));
    }
    let capabilities = capabilities
        .iter()
        .filter_map(|item| Capability::from_name(item))
        .collect();
    let Some(tier) = Tier::from_name(&tier) else {
        return Err(RenderError::UnknownTier(tier));
    };
    Ok((
        Frontmatter {
            name,
            description,
            capabilities,
            tier,
        },
        body,
    ))
}

/// `value` as a mapping, when it is one.
fn mapping(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// `key` from the stage's own entry when it has the key, otherwise from the default; a value
/// that is not a string counts as absent.
fn agent_key(
    own: Option<&Map<String, Value>>,
    default: Option<&Map<String, Value>>,
    key: &str,
) -> Option<String> {
    let value = match own {
        Some(own) if own.contains_key(key) => own.get(key),
        _ => default.and_then(|default| default.get(key)),
    };
    value.and_then(Value::as_str).map(str::to_owned)
}

/// The agent `stage` runs with: each of harness, model and effort from `agents.stages.<stage>`
/// when given there, otherwise from `agents.default`, and the model last from `tier`.
pub fn choose_agent(config: &RepoConfig, stage: &str, tier: Tier) -> Result<Agent, RenderError> {
    let agents = mapping(config.value().get("agents"));
    let default = mapping(agents.and_then(|agents| agents.get("default")));
    let own = mapping(
        agents
            .and_then(|agents| agents.get("stages"))
            .and_then(|stages| stages.get(stage)),
    );
    let Some(harness) = agent_key(own, default, "harness") else {
        return Err(RenderError::NoHarness {
            stage: stage.to_owned(),
        });
    };
    let model = match agent_key(own, default, "model") {
        Some(model) => model,
        None => match tier_model(&harness, tier) {
            Some(model) => model.to_owned(),
            None => return Err(RenderError::NoModel { harness }),
        },
    };
    Ok(Agent {
        harness,
        model,
        effort: agent_key(own, default, "effort"),
    })
}

/// Whether `stage` is ASCII lowercase letters, digits and `-`, starting with a letter.
fn valid_name(stage: &str) -> bool {
    let mut chars = stage.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The bytes at `path` in `commit`, or `None` when it is not a file there.
fn read<V: Vcs>(vcs: &V, commit: &RevId, path: &str) -> Result<Option<Vec<u8>>, RenderError> {
    vcs.read_blob(commit, &RepoPath(path.as_bytes().to_vec()))
        .map_err(|source| RenderError::Store {
            path: path.to_owned(),
            source: Box::new(source),
        })
}

/// Render `stage` from the trusted store at `commit`, a full commit id, with the agent
/// `config` chooses.
pub fn render<V: Vcs>(
    vcs: &V,
    commit: &RevId,
    stage: &str,
    config: &RepoConfig,
) -> Result<RenderedStage, RenderError> {
    if !valid_name(stage) {
        return Err(RenderError::InvalidName(stage.to_owned()));
    }
    let stage_path = format!("{STAGES_DIR}/{stage}.md");
    let Some(stage_bytes) = read(vcs, commit, &stage_path)? else {
        return Err(RenderError::MissingStage { path: stage_path });
    };
    let Some(conventions_bytes) = read(vcs, commit, CONVENTIONS)? else {
        return Err(RenderError::MissingConventions {
            path: CONVENTIONS.to_owned(),
        });
    };
    let (frontmatter, body) = parse(stage, &stage_bytes)?;
    let agent = choose_agent(config, stage, frontmatter.tier)?;
    Ok(RenderedStage {
        frontmatter,
        body,
        agent,
        instructions: commit.clone(),
        stage_file: bind(stage_path, &stage_bytes),
        conventions: bind(CONVENTIONS, &conventions_bytes),
    })
}
