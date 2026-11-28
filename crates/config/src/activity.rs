//! The activity rules' schema, one for the runner's compiled-in defaults,
//! `defaults/activity/rules.yaml`, and a project's `metrics.activity` in `.mintswarm/config.yaml`;
//! [`ActivityRule`] describes it.

use mintswarm_domain::facts::catalogue::Harness;
use serde_json::Value;

use crate::schema::{kind, known, mapping, string, string_list};
use crate::{Error, Path, parse_yaml};

/// One activity rule: what it matches, for which harnesses, and the activity it names.
///
/// The rules have one schema, read by [`load_activity_rules`] for a rules file and by
/// [`load_repo_config`](crate::load_repo_config) for `metrics.activity`. Both sources are a
/// mapping with one key, `rules`, a list of rules:
///
/// ```yaml
/// rules:
///   - { tool: Read, activity: reading }                       # a harness tool, exact or glob
///   - { tool: "Web*", harness: [claude], activity: fetching } # optional harness filter
///   - { command: git, args: [commit], activity: committing }  # program and leading arguments
///   - { command: grep, activity: searching }
///   - { mcp: { server: github, tool: "*" }, activity: fetching }
/// ```
///
/// Each rule is checked at parse:
///
/// - it has exactly one of `tool`, `command` and `mcp`, and `mcp` holds exactly `server` and
///   `tool`;
/// - `args`, a list of patterns, is allowed only with `command`;
/// - `harness`, when present, is a non-empty list of harness names (`claude`, `codex`);
/// - `activity` matches `[a-z][a-z0-9_-]*`;
/// - every other key is an error, at every level.
///
/// Patterns are the plan floor's path globs, matched by the classifier in `mintswarm-rules`:
/// `*` matches any run of characters and `?` one character, neither crossing `/`; `**` matches
/// any run, `/` included; every other character is literal. A pattern matches the whole name or
/// argument, case-sensitively, so `*.py` does not match `scripts/x.py`, while `**.py` does.
///
/// An absent `rules` reads as no rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityRule {
    /// What the rule matches.
    pub matcher: ActivityMatch,
    /// The harnesses the rule applies to; every harness when `None`.
    pub harness: Option<Vec<Harness>>,
    /// The activity, matching `[a-z][a-z0-9_-]*`.
    pub activity: String,
}

/// What an activity rule matches, each part a glob pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityMatch {
    /// `tool`: a harness tool by name.
    Tool(String),
    /// `command` and `args`: a shell program and its leading arguments.
    Command {
        /// `command`, matched against the program's basename.
        program: String,
        /// `args`, matched in order against the leading arguments; empty when absent.
        args: Vec<String>,
    },
    /// `mcp`: an MCP call by server and tool.
    Mcp {
        /// `mcp.server`.
        server: String,
        /// `mcp.tool`.
        tool: String,
    },
}

/// The keys a rule may hold.
const RULE_KEYS: [&str; 6] = ["tool", "command", "args", "mcp", "harness", "activity"];
/// The keys of which a rule holds exactly one.
const MATCHERS: [&str; 3] = ["tool", "command", "mcp"];

/// Parse and check an activity rules file, such as `defaults/activity/rules.yaml`.
pub fn load_activity_rules(text: &str) -> Result<Vec<ActivityRule>, Error> {
    check_activity(&parse_yaml(text)?, &Path::root())
}

/// Check a `{ rules: [...] }` mapping at `path` and return its rules, in file order.
pub(crate) fn check_activity(value: &Value, path: &Path) -> Result<Vec<ActivityRule>, Error> {
    let map = mapping(value, path)?;
    known(map, path, &["rules"])?;
    let Some(rules) = map.get("rules") else {
        return Ok(Vec::new());
    };
    let path = path.key("rules");
    let items = rules
        .as_array()
        .ok_or_else(|| Error::schema(&path, format!("must be a list, not {}", kind(rules))))?;
    items
        .iter()
        .enumerate()
        .map(|(index, item)| check_rule(item, &path.index(index)))
        .collect()
}

fn check_rule(value: &Value, path: &Path) -> Result<ActivityRule, Error> {
    let map = mapping(value, path)?;
    known(map, path, &RULE_KEYS)?;
    let present: Vec<&str> = MATCHERS
        .into_iter()
        .filter(|key| map.contains_key(*key))
        .collect();
    let [matcher] = present[..] else {
        return Err(Error::schema(
            path,
            "must have exactly one of tool, command and mcp",
        ));
    };
    if matcher != "command" && map.contains_key("args") {
        return Err(Error::schema(
            &path.key("args"),
            "is allowed only with command",
        ));
    }
    let matcher = match matcher {
        "tool" => ActivityMatch::Tool(pattern(map, "tool", path)?),
        "command" => ActivityMatch::Command {
            program: pattern(map, "command", path)?,
            args: match map.get("args") {
                Some(args) => string_list(args, &path.key("args"))?,
                None => Vec::new(),
            },
        },
        _ => {
            let mcp_path = path.key("mcp");
            let mcp = map.get("mcp").map_or(&Value::Null, |mcp| mcp);
            let mcp_map = mapping(mcp, &mcp_path)?;
            known(mcp_map, &mcp_path, &["server", "tool"])?;
            ActivityMatch::Mcp {
                server: pattern(mcp_map, "server", &mcp_path)?,
                tool: pattern(mcp_map, "tool", &mcp_path)?,
            }
        }
    };
    let harness = match map.get("harness") {
        Some(harness) => Some(check_harness(harness, &path.key("harness"))?),
        None => None,
    };
    let activity_path = path.key("activity");
    let activity = match map.get("activity") {
        Some(activity) => string(activity, &activity_path)?,
        None => return Err(Error::schema(&activity_path, "is missing")),
    };
    if !is_activity(activity) {
        return Err(Error::schema(
            &activity_path,
            format!("must match [a-z][a-z0-9_-]*, not {activity:?}"),
        ));
    }
    Ok(ActivityRule {
        matcher,
        harness,
        activity: activity.to_owned(),
    })
}

/// The string pattern under `key`, which must be present.
fn pattern(map: &serde_json::Map<String, Value>, key: &str, path: &Path) -> Result<String, Error> {
    let path = path.key(key);
    match map.get(key) {
        Some(value) => string(value, &path).map(str::to_owned),
        None => Err(Error::schema(&path, "is missing")),
    }
}

fn check_harness(value: &Value, path: &Path) -> Result<Vec<Harness>, Error> {
    let items = value
        .as_array()
        .ok_or_else(|| Error::schema(path, format!("must be a list, not {}", kind(value))))?;
    if items.is_empty() {
        return Err(Error::schema(path, "must not be empty"));
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let path = path.index(index);
            let name = string(item, &path)?;
            serde_json::from_value::<Harness>(Value::String(name.to_owned()))
                .map_err(|_| Error::schema(&path, format!("is not a harness: {name:?}")))
        })
        .collect()
}

/// Whether `activity` matches `[a-z][a-z0-9_-]*`.
fn is_activity(activity: &str) -> bool {
    let mut chars = activity.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::is_activity;

    #[test]
    fn activity_names_follow_the_pattern() {
        for good in ["reading", "a", "code-review", "x_1"] {
            assert!(is_activity(good), "{good}");
        }
        for bad in ["", "Reading", "1x", "-a", "a b", "é"] {
            assert!(!is_activity(bad), "{bad}");
        }
    }
}
