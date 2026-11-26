//! The arguments of `msw-vcs`: parsing argv into a [`Request`], quoting, and the messages of
//! exit codes 2 and 3, as `.mintswarm/dogfood/msw-vcs.md` specifies them.

use mintswarm_domain::vcs::RepoPath;

/// The usage line, the message of a usage error.
pub const USAGE: &[u8] = b"usage: msw-vcs log [REV | REV..REV] [-- PATH...] \
| msw-vcs diff [--numstat | --name-only | --raw] (REV REV | --job) [-- PATH...] \
| msw-vcs show REV[:PATH]";

/// The message for more than one output form.
pub const ONE_FORM: &[u8] = b"at most one of --numstat, --name-only and --raw";

/// The names of the job's three revisions.
const NAMES: [(&[u8], Rev); 3] = [
    (b"base", Rev::Base),
    (b"head", Rev::Head),
    (b"merge-base", Rev::MergeBase),
];

/// The separator before paths.
const PATHS: &[u8] = b"--";

/// The flag of the job's change.
const JOB: &[u8] = b"--job";

/// A call that fails: its exit code, and the message written after `msw-vcs: `.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fail {
    /// The exit code.
    pub exit: u8,
    /// The message, without the `msw-vcs: ` prefix and the newline.
    pub message: Vec<u8>,
}

impl Fail {
    /// Exit code 2 with the usage line.
    pub fn usage() -> Fail {
        Fail {
            exit: 2,
            message: USAGE.to_vec(),
        }
    }

    /// Exit code 2: `refused revision: <arg>`.
    pub fn refused_revision(arg: &[u8]) -> Fail {
        Fail::quoted(2, b"refused revision: ", arg)
    }

    /// Exit code 2: `refused path: <arg>`.
    pub fn refused_path(arg: &[u8]) -> Fail {
        Fail::quoted(2, b"refused path: ", arg)
    }

    /// Exit code 3: `unknown revision or path: <arg>`.
    pub fn unknown(arg: &[u8]) -> Fail {
        Fail::quoted(3, b"unknown revision or path: ", arg)
    }

    /// Exit code 4, a job set-up error, with `message`.
    pub fn setup(message: &str) -> Fail {
        Fail {
            exit: 4,
            message: message.as_bytes().to_vec(),
        }
    }

    /// Exit code 5: `vcs failure`.
    pub fn vcs_failure() -> Fail {
        Fail {
            exit: 5,
            message: b"vcs failure".to_vec(),
        }
    }

    fn quoted(exit: u8, prefix: &[u8], arg: &[u8]) -> Fail {
        let mut message = prefix.to_vec();
        message.extend(quote(arg));
        Fail { exit, message }
    }

    /// The whole line written to stderr: `msw-vcs: <message>` and a newline.
    pub fn line(&self) -> Vec<u8> {
        let mut line = b"msw-vcs: ".to_vec();
        line.extend(&self.message);
        line.push(b'\n');
        line
    }
}

/// A revision as given: one of the job's names, or a full commit id still to be checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rev {
    /// `base`.
    Base,
    /// `head`.
    Head,
    /// `merge-base`.
    MergeBase,
    /// 40 or 64 lower-case hexadecimal digits.
    Id(String),
}

/// The output form of `diff`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// The unified diff, the default.
    Patch,
    /// `--numstat`.
    Numstat,
    /// `--name-only`.
    NameOnly,
    /// `--raw`.
    Raw,
}

/// The revisions of `log`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRange {
    /// The left side of `A..B`; `None` for a single revision.
    pub from: Option<Rev>,
    /// The single revision, or the right side of `A..B`.
    pub to: Rev,
    /// The whole argument, for the message of an unknown revision.
    pub arg: Vec<u8>,
}

/// A parsed call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// `log [REV | REV..REV] [-- PATH...]`; no range is `merge-base..head`.
    Log {
        /// The revisions given, if any.
        range: Option<LogRange>,
        /// The paths after `--`.
        paths: Vec<RepoPath>,
    },
    /// `diff [FORM] (REV REV | --job) [-- PATH...]`.
    Diff {
        /// The output form.
        form: Form,
        /// The old and the new revision with their arguments; `None` for `--job`.
        revs: Option<[(Rev, Vec<u8>); 2]>,
        /// The paths after `--`.
        paths: Vec<RepoPath>,
    },
    /// `show REV[:PATH]`.
    Show {
        /// The revision.
        rev: Rev,
        /// The path after `:`, empty for the whole tree; `None` without `:`.
        path: Option<Vec<u8>>,
        /// The whole argument, for the message of an unknown revision or path.
        arg: Vec<u8>,
    },
}

/// Parse argv, without the program name, into a request; the first argument check that fails
/// decides, in the shim's order.
pub fn parse(argv: &[Vec<u8>]) -> Result<Request, Fail> {
    let Some((command, args)) = argv.split_first() else {
        return Err(Fail::usage());
    };
    match command.as_slice() {
        b"log" => parse_log(args),
        b"diff" => parse_diff(args),
        b"show" => parse_show(args),
        _ => Err(Fail::usage()),
    }
}

/// A revision in one of the four forms, or `None`.
fn revision(token: &[u8]) -> Option<Rev> {
    if let Some((_, rev)) = NAMES.iter().find(|(name, _)| *name == token) {
        return Some(rev.clone());
    }
    let full = matches!(token.len(), 40 | 64)
        && token
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    full.then(|| String::from_utf8_lossy(token).into_owned())
        .map(Rev::Id)
}

/// The revision `token`, or a refusal quoting `arg`.
fn check_revision(token: &[u8], arg: &[u8]) -> Result<Rev, Fail> {
    revision(token).ok_or_else(|| Fail::refused_revision(arg))
}

/// Whether a path is relative, with no empty, `.` or `..` component.
fn path_ok(path: &[u8]) -> bool {
    !path.starts_with(b"/")
        && path
            .split(|&byte| byte == b'/')
            .all(|part| !matches!(part, b"" | b"." | b".."))
}

/// The arguments before the first `--`, and the paths after it.
fn split_paths(args: &[Vec<u8>]) -> (&[Vec<u8>], &[Vec<u8>]) {
    match args.iter().position(|arg| arg == PATHS) {
        Some(at) => (&args[..at], &args[at + 1..]),
        None => (args, &[]),
    }
}

fn check_paths(paths: &[Vec<u8>]) -> Result<Vec<RepoPath>, Fail> {
    paths
        .iter()
        .map(|path| {
            if path_ok(path) {
                Ok(RepoPath(path.clone()))
            } else {
                Err(Fail::refused_path(path))
            }
        })
        .collect()
}

/// The non-overlapping occurrences of `needle` in `haystack`, as Python's `bytes.count` counts.
fn count(haystack: &[u8], needle: &[u8]) -> usize {
    let mut found = 0;
    let mut at = 0;
    while let Some(offset) = find(&haystack[at..], needle) {
        found += 1;
        at += offset + needle.len();
    }
    found
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_log(args: &[Vec<u8>]) -> Result<Request, Fail> {
    let (revs, paths) = split_paths(args);
    let mut range = None;
    for arg in revs {
        if find(arg, b"...").is_some() || count(arg, b"..") > 1 {
            return Err(Fail::refused_revision(arg));
        }
        range = Some(match find(arg, b"..") {
            Some(at) => LogRange {
                from: Some(check_revision(&arg[..at], arg)?),
                to: check_revision(&arg[at + 2..], arg)?,
                arg: arg.clone(),
            },
            None => LogRange {
                from: None,
                to: check_revision(arg, arg)?,
                arg: arg.clone(),
            },
        });
    }
    if revs.len() > 1 {
        return Err(Fail::usage());
    }
    Ok(Request::Log {
        range,
        paths: check_paths(paths)?,
    })
}

fn parse_diff(args: &[Vec<u8>]) -> Result<Request, Fail> {
    let (mut rest, paths) = split_paths(args);
    let mut form = None;
    let mut job = false;
    while let Some((flag, tail)) = rest.split_first() {
        let flag = match flag.as_slice() {
            b"--numstat" => Some(Form::Numstat),
            b"--name-only" => Some(Form::NameOnly),
            b"--raw" => Some(Form::Raw),
            JOB => None,
            _ => break,
        };
        rest = tail;
        match flag {
            None if job => return Err(Fail::usage()),
            None => job = true,
            Some(_) if form.is_some() => {
                return Err(Fail {
                    exit: 2,
                    message: ONE_FORM.to_vec(),
                });
            }
            Some(flag) => form = Some(flag),
        }
    }
    let revs = rest
        .iter()
        .map(|arg| check_revision(arg, arg).map(|rev| (rev, arg.clone())))
        .collect::<Result<Vec<_>, _>>()?;
    let revs = match (job, <[_; 2]>::try_from(revs)) {
        (true, Err(revs)) if revs.is_empty() => None,
        (false, Ok(revs)) => Some(revs),
        _ => return Err(Fail::usage()),
    };
    Ok(Request::Diff {
        form: form.unwrap_or(Form::Patch),
        revs,
        paths: check_paths(paths)?,
    })
}

fn parse_show(args: &[Vec<u8>]) -> Result<Request, Fail> {
    if args.iter().any(|arg| arg == PATHS) {
        return Err(Fail::usage());
    }
    let mut requests = Vec::with_capacity(1);
    for arg in args {
        let (rev, path) = match arg.iter().position(|&byte| byte == b':') {
            Some(at) => (&arg[..at], Some(&arg[at + 1..])),
            None => (arg.as_slice(), None),
        };
        let rev = check_revision(rev, arg)?;
        if let Some(path) = path
            && !path.is_empty()
            && !path_ok(path)
        {
            return Err(Fail::refused_path(arg));
        }
        requests.push(Request::Show {
            rev,
            path: path.map(<[u8]>::to_vec),
            arg: arg.clone(),
        });
    }
    match (requests.pop(), requests.is_empty()) {
        (Some(request), true) => Ok(request),
        _ => Err(Fail::usage()),
    }
}

/// git's C-style quoting under `core.quotePath=false`: only control bytes, DEL, `"` and `\`
/// force it, and an empty argument is written `""`.
pub fn quote(bytes: &[u8]) -> Vec<u8> {
    let special = |byte: u8| byte < 0x20 || matches!(byte, b'"' | b'\\' | 0x7f);
    if !bytes.is_empty() && !bytes.iter().copied().any(special) {
        return bytes.to_vec();
    }
    let mut out = vec![b'"'];
    for &byte in bytes {
        match byte {
            7 => out.extend(b"\\a"),
            8 => out.extend(b"\\b"),
            9 => out.extend(b"\\t"),
            10 => out.extend(b"\\n"),
            11 => out.extend(b"\\v"),
            12 => out.extend(b"\\f"),
            13 => out.extend(b"\\r"),
            b'"' => out.extend(b"\\\""),
            b'\\' => out.extend(b"\\\\"),
            byte if special(byte) => out.extend(format!("\\{byte:03o}").as_bytes()),
            byte => out.push(byte),
        }
    }
    out.push(b'"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<Vec<u8>> {
        args.iter().map(|arg| arg.as_bytes().to_vec()).collect()
    }

    fn fail(args: &[&str]) -> Fail {
        parse(&argv(args)).unwrap_err()
    }

    #[test]
    fn quote_follows_git() {
        assert_eq!(quote(b"plain caf\xc3\xa9"), b"plain caf\xc3\xa9");
        assert_eq!(quote(b""), b"\"\"");
        assert_eq!(quote(b"we\"ird\tname"), b"\"we\\\"ird\\tname\"");
        assert_eq!(quote(b"a\x01\x7f\\"), b"\"a\\001\\177\\\\\"");
    }

    #[test]
    fn checks_run_in_the_shim_order() {
        // a refused revision before the count of revisions
        assert_eq!(
            fail(&["log", "topic", "base"]).message,
            b"refused revision: topic"
        );
        assert_eq!(fail(&["log", "base", "head"]), Fail::usage());
        // the count of revisions before a refused path
        assert_eq!(fail(&["diff", "base", "--", ".."]), Fail::usage());
        assert_eq!(fail(&["diff", "--job", "--job"]), Fail::usage());
        assert_eq!(fail(&["diff", "--raw", "--raw", "--job"]).message, ONE_FORM);
        assert_eq!(fail(&["show", "base", "head"]), Fail::usage());
        assert_eq!(fail(&["show"]), Fail::usage());
        assert_eq!(
            fail(&["log", "a..b..c"]).message,
            b"refused revision: a..b..c"
        );
    }

    #[test]
    fn parses_each_command() {
        let id = "a".repeat(40);
        assert_eq!(
            parse(&argv(&["log", &format!("base..{id}"), "--", "docs"])).unwrap(),
            Request::Log {
                range: Some(LogRange {
                    from: Some(Rev::Base),
                    to: Rev::Id(id.clone()),
                    arg: format!("base..{id}").into_bytes(),
                }),
                paths: vec![RepoPath(b"docs".to_vec())],
            }
        );
        assert_eq!(
            parse(&argv(&["diff", "--job", "--numstat"])).unwrap(),
            Request::Diff {
                form: Form::Numstat,
                revs: None,
                paths: Vec::new()
            }
        );
        assert_eq!(
            parse(&argv(&["show", "head:"])).unwrap(),
            Request::Show {
                rev: Rev::Head,
                path: Some(Vec::new()),
                arg: b"head:".to_vec()
            }
        );
    }
}
