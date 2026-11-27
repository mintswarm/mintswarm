//! The argument rules `plan-approval/floor.py` and `decision.py` share, for
//! [`super::plan_floor`] and [`super::plan_decision`]: each flag at most once, as `--name value`;
//! no unknown flag, empty value or value that is a flag name; every required flag given; a
//! `--base` that does not start with `-`; and, where the tool asks for it, an `--issue` that is a
//! positive decimal integer. The messages are the scripts', with the tool's usage line.
//!
//! One intended difference from `parse_args`: an argument that is not UTF-8 is refused, exit 2
//! with `arguments must be UTF-8; <usage>`, where Python on POSIX takes it through
//! surrogateescape and opens the path. The values are kept as `str`, as every reader of them
//! takes text.

use std::ffi::OsString;

use mintswarm_rules::exit::Unusable;
use mintswarm_rules::py::repr_str;

/// A tool's flags and usage line.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// The flags that must be given, in the order a missing one is named.
    pub required: &'static [&'static str],
    /// The flags that may be given.
    pub optional: &'static [&'static str],
    /// The usage line, appended to the refusals that name it.
    pub usage: &'static str,
    /// Whether `--issue` must be a positive decimal integer.
    pub issue: bool,
}

impl Spec {
    fn knows(&self, name: &str) -> bool {
        self.required.contains(&name) || self.optional.contains(&name)
    }
}

/// The arguments given, each flag with its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    values: Vec<(String, String)>,
}

impl Args {
    /// The value of `name`, when it was given.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(flag, _)| flag == name)
            .map(|(_, value)| value.as_str())
    }

    /// The value of the required flag `name`.
    ///
    /// # Panics
    ///
    /// When `name` is not one of the spec's required flags, which [`parse`] has checked.
    pub fn required(&self, name: &str) -> &str {
        self.get(name)
            .unwrap_or_else(|| panic!("{name} is a required flag"))
    }

    /// `--components` split on `,` as Python's `str.split(",")` splits it, so an empty value is
    /// one empty name.
    pub fn components(&self) -> Vec<String> {
        self.get("--components")
            .map(|names| names.split(',').map(str::to_owned).collect())
            .unwrap_or_default()
    }
}

/// `argv` read by `spec`'s rules, in `parse_args`' order.
pub fn parse(argv: &[OsString], spec: &Spec) -> Result<Args, Unusable> {
    let usage = spec.usage;
    let argv: Vec<&str> = argv
        .iter()
        .map(|arg| arg.to_str())
        .collect::<Option<_>>()
        .ok_or_else(|| Unusable::new(format!("arguments must be UTF-8; {usage}")))?;
    let mut values: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let name = argv[i];
        if !spec.knows(name) {
            return Err(Unusable::new(format!(
                "unknown argument {}; {usage}",
                repr_str(name)
            )));
        }
        if values.iter().any(|(flag, _)| flag == name) {
            return Err(Unusable::new(format!("{name} given twice; {usage}")));
        }
        let value = match argv.get(i + 1) {
            Some(value) if !value.is_empty() && !spec.knows(value) => *value,
            _ => return Err(Unusable::new(format!("{name} needs a value; {usage}"))),
        };
        values.push((name.to_owned(), value.to_owned()));
        i += 2;
    }
    let args = Args { values };
    let missing: Vec<&str> = spec
        .required
        .iter()
        .copied()
        .filter(|name| args.get(name).is_none())
        .collect();
    if !missing.is_empty() {
        return Err(Unusable::new(format!(
            "missing {}; {usage}",
            missing.join(", ")
        )));
    }
    if spec.issue && !is_issue(args.required("--issue")) {
        return Err(Unusable::new(format!(
            "--issue must be a positive decimal integer: {}",
            repr_str(args.required("--issue"))
        )));
    }
    let base = args.required("--base");
    if base.starts_with('-') {
        return Err(Unusable::new(format!(
            "--base must not start with '-': {}",
            repr_str(base)
        )));
    }
    Ok(args)
}

/// Whether `text` matches `[1-9][0-9]*`.
fn is_issue(text: &str) -> bool {
    let mut bytes = text.bytes();
    matches!(bytes.next(), Some(b'1'..=b'9')) && bytes.all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    const FLOOR: Spec = Spec {
        required: &["--plan", "--components", "--base"],
        optional: &["--issue-document", "--analysis", "--screen"],
        usage: "usage: floor",
        issue: false,
    };

    const DECISION: Spec = Spec {
        required: &["--issue", "--components", "--base"],
        optional: &["--screen"],
        usage: "usage: decision",
        issue: true,
    };

    fn argv(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn refusal(args: &[&str], spec: &Spec) -> String {
        parse(&argv(args), spec)
            .expect_err("the arguments are refused")
            .message()
            .to_owned()
    }

    const PLAIN: &[&str] = &["--plan", "p.md", "--components", "docs", "--base", "HEAD"];

    #[test]
    fn plain_arguments_parse() {
        let args = parse(&argv(PLAIN), &FLOOR).expect("usable arguments");
        assert_eq!(args.get("--plan"), Some("p.md"));
        assert_eq!(args.get("--screen"), None);
        assert_eq!(args.components(), ["docs"]);
        assert_eq!(args.required("--base"), "HEAD");
    }

    #[test]
    fn components_split_as_python_splits() {
        let split = |value: &str| {
            parse(
                &argv(&["--plan", "p", "--components", value, "--base", "b"]),
                &FLOOR,
            )
            .expect("usable arguments")
            .components()
        };
        assert_eq!(split("docs,"), ["docs", ""]);
        assert_eq!(split(",docs"), ["", "docs"]);
        assert_eq!(split("a,b"), ["a", "b"]);
    }

    #[test]
    fn each_refusal_has_the_scripts_message() {
        assert_eq!(
            refusal(&[], &FLOOR),
            "missing --plan, --components, --base; usage: floor"
        );
        assert_eq!(
            refusal(&[PLAIN, &["--extra", "x"]].concat(), &FLOOR),
            "unknown argument '--extra'; usage: floor"
        );
        assert_eq!(
            refusal(&[PLAIN, &["--plan", "q"]].concat(), &FLOOR),
            "--plan given twice; usage: floor"
        );
        assert_eq!(
            refusal(&PLAIN[..5], &FLOOR),
            "--base needs a value; usage: floor"
        );
        assert_eq!(
            refusal(&[PLAIN, &["--screen", ""]].concat(), &FLOOR),
            "--screen needs a value; usage: floor"
        );
        assert_eq!(
            refusal(&[PLAIN, &["--screen", "--plan"]].concat(), &FLOOR),
            "--screen needs a value; usage: floor"
        );
        assert_eq!(
            refusal(
                &["--plan", "p", "--components", "d", "--base", "-HEAD"],
                &FLOOR
            ),
            "--base must not start with '-': '-HEAD'"
        );
        // --issue is a flag of the decision only
        assert_eq!(
            refusal(&[PLAIN, &["--issue", "3"]].concat(), &FLOOR),
            "unknown argument '--issue'; usage: floor"
        );
        // a flag name of the other tool is a value here
        let args = parse(&argv(&[PLAIN, &["--screen", "--issue"]].concat()), &FLOOR)
            .expect("usable arguments");
        assert_eq!(args.get("--screen"), Some("--issue"));
        assert_eq!(
            refusal(&["--components", "d"], &DECISION),
            "missing --issue, --base; usage: decision"
        );
    }

    #[test]
    fn the_issue_must_be_a_positive_decimal_integer() {
        let with = |issue: &str| {
            parse(
                &argv(&["--issue", issue, "--components", "d", "--base", "b"]),
                &DECISION,
            )
        };
        assert!(with("34").is_ok());
        assert!(with("1").is_ok());
        for bad in ["0", "034", "-3", "abc", "3a", "٣"] {
            assert_eq!(
                with(bad).expect_err("refused").message(),
                format!(
                    "--issue must be a positive decimal integer: {}",
                    repr_str(bad)
                ),
                "{bad}"
            );
        }
        // the issue is checked before the base
        assert_eq!(
            parse(
                &argv(&["--issue", "0", "--components", "d", "--base", "-b"]),
                &DECISION
            )
            .expect_err("refused")
            .message(),
            "--issue must be a positive decimal integer: '0'"
        );
    }

    #[test]
    fn a_non_utf8_argument_is_refused() {
        let mut args = argv(PLAIN);
        args[1] = OsString::from_vec(b"p\xff.md".to_vec());
        assert_eq!(
            parse(&args, &FLOOR).expect_err("refused").message(),
            "arguments must be UTF-8; usage: floor"
        );
    }
}
