//! The activity classifier: which activity a tool call belongs to, by the activity rules.
//!
//! The rules are the runner's compiled-in defaults and a project's `metrics.activity.rules`,
//! both in the schema of [`ActivityRule`]. A call is matched only by rules of its own kind:
//!
//! - a harness tool by `tool` rules, on its name;
//! - a shell command by `command` rules, on the program and leading arguments that
//!   [`shell::normalise`] gives;
//! - an MCP call by `mcp` rules, on its server and tool.
//!
//! Every pattern is a [`Glob`], case-sensitive; a pattern without `*` or `?` is exact. A rule
//! with a harness filter applies only to those harnesses. Of the matching rules, the first
//! difference in this order decides:
//!
//! 1. more argument patterns;
//! 2. more exact patterns among the name and the arguments;
//! 3. project over default;
//! 4. earlier in its file.
//!
//! A call no rule matches is [`OTHER`] with source [`ActivitySource::None`], and a shell
//! command the normaliser cannot read is [`OTHER`] with source [`ActivitySource::Unparsed`].

pub mod shell;

use mintswarm_config::{ActivityMatch, ActivityRule};
use mintswarm_domain::facts::catalogue::{ActivitySource, Harness};

use crate::glob::Glob;

/// The activity of a call no rule classifies.
pub const OTHER: &str = "other";

/// The base set of activities the default rules use; a project may add its own.
pub const BASE_ACTIVITIES: [&str; 9] = [
    "reading",
    "searching",
    "writing",
    "building",
    "testing",
    "running",
    "committing",
    "fetching",
    OTHER,
];

/// A tool call, as the rules see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// A harness tool, by name.
    Tool(String),
    /// The harness's shell tool, with its command line.
    Shell(String),
    /// An MCP call.
    Mcp {
        /// The MCP server.
        server: String,
        /// The tool on that server.
        tool: String,
    },
}

impl Call {
    /// The call a harness names `name`, with `command` the command line when it is the
    /// harness's shell tool.
    ///
    /// For Claude Code, `Bash` is the shell tool, and `mcp__<server>__<tool>` is an MCP call,
    /// split at the first `__` after the prefix. For Codex, `shell` and `local_shell` are the
    /// shell tool. Every other name is a harness tool, and a shell tool without a command line
    /// has an empty one.
    pub fn from_harness(harness: Harness, name: &str, command: Option<&str>) -> Call {
        let shell = match harness {
            Harness::Claude => name == "Bash",
            Harness::Codex => matches!(name, "shell" | "local_shell"),
        };
        if shell {
            return Call::Shell(command.unwrap_or_default().to_owned());
        }
        if harness == Harness::Claude
            && let Some(rest) = name.strip_prefix("mcp__")
            && let Some((server, tool)) = rest.split_once("__")
            && !server.is_empty()
            && !tool.is_empty()
        {
            return Call::Mcp {
                server: server.to_owned(),
                tool: tool.to_owned(),
            };
        }
        Call::Tool(name.to_owned())
    }
}

/// A call's activity and which rules gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    /// The activity, [`OTHER`] when no rule matched.
    pub activity: String,
    /// The rules' origin, or why none applied.
    pub source: ActivitySource,
}

/// The defaults and a project's rules, compiled for matching.
#[derive(Debug, Clone)]
pub struct Rules {
    rules: Vec<Compiled>,
}

/// One compiled rule, with what the precedence needs.
#[derive(Debug, Clone)]
struct Compiled {
    matcher: Matcher,
    harness: Option<Vec<Harness>>,
    activity: String,
    /// [`ActivitySource::Default`] or [`ActivitySource::Project`].
    origin: ActivitySource,
    /// The rule's index in its file.
    index: usize,
    /// The number of argument patterns.
    args: usize,
    /// The number of exact patterns among the name and the arguments.
    exact: usize,
}

#[derive(Debug, Clone)]
enum Matcher {
    Tool(Glob),
    Command { program: Glob, args: Vec<Glob> },
    Mcp { server: Glob, tool: Glob },
}

impl Compiled {
    fn new(rule: &ActivityRule, origin: ActivitySource, index: usize) -> Self {
        let patterns: Vec<&str> = match &rule.matcher {
            ActivityMatch::Tool(name) => vec![name],
            ActivityMatch::Command { program, args } => std::iter::once(program.as_str())
                .chain(args.iter().map(String::as_str))
                .collect(),
            ActivityMatch::Mcp { server, tool } => vec![server, tool],
        };
        let matcher = match &rule.matcher {
            ActivityMatch::Tool(name) => Matcher::Tool(Glob::new(name)),
            ActivityMatch::Command { program, args } => Matcher::Command {
                program: Glob::new(program),
                args: args.iter().map(|arg| Glob::new(arg)).collect(),
            },
            ActivityMatch::Mcp { server, tool } => Matcher::Mcp {
                server: Glob::new(server),
                tool: Glob::new(tool),
            },
        };
        let args = match &rule.matcher {
            ActivityMatch::Command { args, .. } => args.len(),
            _ => 0,
        };
        Compiled {
            matcher,
            harness: rule.harness.clone(),
            activity: rule.activity.clone(),
            origin,
            index,
            args,
            exact: patterns.iter().filter(|pattern| is_exact(pattern)).count(),
        }
    }

    /// The precedence key: the larger key wins.
    fn key(&self) -> (usize, usize, bool, std::cmp::Reverse<usize>) {
        (
            self.args,
            self.exact,
            self.origin == ActivitySource::Project,
            std::cmp::Reverse(self.index),
        )
    }

    fn applies_to(&self, harness: Harness) -> bool {
        self.harness
            .as_ref()
            .is_none_or(|harnesses| harnesses.contains(&harness))
    }
}

/// A call in the form the matchers compare, the shell command already normalised.
enum Target<'a> {
    Tool(&'a str),
    Command(&'a shell::Normalised),
    Mcp { server: &'a str, tool: &'a str },
}

impl Matcher {
    fn matches(&self, target: &Target<'_>) -> bool {
        match (self, target) {
            (Matcher::Tool(glob), Target::Tool(name)) => glob.matches(name),
            (Matcher::Command { program, args }, Target::Command(normalised)) => {
                program.matches(&normalised.program)
                    && args.len() <= normalised.args.len()
                    && args
                        .iter()
                        .zip(&normalised.args)
                        .all(|(glob, arg)| glob.matches(arg))
            }
            (Matcher::Mcp { server, tool }, Target::Mcp { server: s, tool: t }) => {
                server.matches(s) && tool.matches(t)
            }
            _ => false,
        }
    }
}

impl Rules {
    /// The `defaults` and the `project`'s rules, each in its file order.
    pub fn new(defaults: &[ActivityRule], project: &[ActivityRule]) -> Self {
        let tagged = |rules: &[ActivityRule], origin| {
            rules
                .iter()
                .enumerate()
                .map(move |(index, rule)| Compiled::new(rule, origin, index))
                .collect::<Vec<_>>()
        };
        let mut rules = tagged(defaults, ActivitySource::Default);
        rules.extend(tagged(project, ActivitySource::Project));
        Rules { rules }
    }

    /// The activity of `call` made through `harness`.
    pub fn classify(&self, harness: Harness, call: &Call) -> Classification {
        let normalised;
        let target = match call {
            Call::Tool(name) => Target::Tool(name),
            Call::Shell(command) => match shell::normalise(command) {
                Ok(result) => {
                    normalised = result;
                    Target::Command(&normalised)
                }
                Err(shell::ShellUnreadable) => {
                    return Classification {
                        activity: OTHER.to_owned(),
                        source: ActivitySource::Unparsed,
                    };
                }
            },
            Call::Mcp { server, tool } => Target::Mcp { server, tool },
        };
        let winner = self
            .rules
            .iter()
            .filter(|rule| rule.applies_to(harness) && rule.matcher.matches(&target))
            .max_by_key(|rule| rule.key());
        match winner {
            Some(rule) => Classification {
                activity: rule.activity.clone(),
                source: rule.origin,
            },
            None => Classification {
                activity: OTHER.to_owned(),
                source: ActivitySource::None,
            },
        }
    }
}

/// Whether `pattern` holds no glob character.
fn is_exact(pattern: &str) -> bool {
    !pattern.contains(['*', '?'])
}
