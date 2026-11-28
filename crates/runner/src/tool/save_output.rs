//! `mintswarm-runner tool save-output [--text] INPUT OUTPUT.md`, the Rust port of
//! `.mintswarm/dogfood/save-output.py`.
//!
//! It reads INPUT, a Claude Code transcript or, with `--text`, the reply itself, captures the
//! document and the `msw-result` block through [`mintswarm_rules::output`], and writes OUTPUT.md
//! and its result record all or none. The operands, stdout, the exit codes and the saved bytes
//! follow `save-output.py`; the fault text is `mintswarm-rules`' own, and an unreadable INPUT or
//! an unwritable OUTPUT is reported in this module's words, never the operating system's.
//!
//! Exit 1 and exit 2 write and delete nothing, so an OUTPUT.md from an earlier run stays as it
//! was, as with `save-output.py`.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use mintswarm_rules::exit::{Outcome, Unusable, Verdict};
use mintswarm_rules::io_phrase;
use mintswarm_rules::markdown::py_rstrip;
use mintswarm_rules::output::{capture, capture_transcript};

/// The usage line printed for operands the tool does not accept.
pub const USAGE: &str = "usage: mintswarm-runner tool save-output [--text] INPUT OUTPUT.md";

/// Where the result record of `output` goes: its `.md` suffix replaced by `.result.json`, or
/// `.result.json` appended. `None` when `output` names no file, as `/` or `..` do.
pub fn record_path(output: &Path) -> Option<PathBuf> {
    let name = output.file_name()?;
    if output.extension() == Some(OsStr::new("md")) {
        return Some(output.with_extension("result.json"));
    }
    if name == ".md" {
        // a dot file has no extension, but save-output.py still drops the suffix
        return Some(output.with_file_name(".result.json"));
    }
    let mut record = name.to_os_string();
    record.push(".result.json");
    Some(output.with_file_name(record))
}

/// `output` normalised as `pathlib.Path` does on POSIX: a trailing `/`, repeated `/` and `.`
/// components dropped, so `out/review.md/` is `out/review.md` and `out/.` is `out`.
pub fn normalised(output: &Path) -> PathBuf {
    output.components().collect()
}

/// Writes every `(path, text)` all or none: each text to a temporary file beside its path,
/// named `.<name>.` and a random suffix, then each temporary renamed onto its path. Parent
/// directories are created as needed. On any error, the temporaries and the files already renamed
/// into place are removed, and the error is returned.
pub fn write_both(writes: &[(PathBuf, &str)]) -> io::Result<()> {
    let mut temps = Vec::with_capacity(writes.len());
    // a temporary is removed when it is dropped, so an early return leaves none behind
    for (path, text) in writes {
        let name = path.file_name().ok_or(ErrorKind::InvalidInput)?;
        let dir = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        fs::create_dir_all(dir)?;
        let mut prefix = OsString::from(".");
        prefix.push(name);
        prefix.push(".");
        let mut temp = tempfile::Builder::new().prefix(&prefix).tempfile_in(dir)?;
        temp.write_all(text.as_bytes())?;
        temps.push(temp);
    }
    let mut done: Vec<&Path> = Vec::with_capacity(writes.len());
    for ((path, _), temp) in writes.iter().zip(temps) {
        if let Err(failed) = temp.persist(path) {
            for placed in done {
                let _ = fs::remove_file(placed);
            }
            return Err(failed.error);
        }
        done.push(path);
    }
    Ok(())
}

/// Runs the tool on its operands, the arguments after `tool save-output` exactly as given,
/// writing what it reports to `stdout` and `stderr`. Failures to write either are ignored.
pub fn run(operands: &[OsString], stdout: &mut impl Write, stderr: &mut impl Write) -> Outcome {
    let text_mode = operands.first().is_some_and(|first| first == "--text");
    let args = if text_mode { &operands[1..] } else { operands };
    let [source, output] = args else {
        return unusable(stderr, USAGE);
    };
    if args
        .iter()
        .any(|arg| arg.as_encoded_bytes().starts_with(b"--"))
    {
        return unusable(stderr, USAGE);
    }
    let shown_source = source.to_string_lossy();
    let shown_output = output.to_string_lossy();

    let text = match read_text(Path::new(source)) {
        Ok(text) => text,
        Err(why) => {
            return unusable(
                stderr,
                &format!("save-output: cannot read {shown_source}: {why}"),
            );
        }
    };
    let (message, result) = if text_mode {
        (None, capture(&text))
    } else {
        let found = capture_transcript(&text);
        (found.message.map(|m| m.describe()), found.result)
    };
    let captured = match result {
        Ok(captured) => captured,
        Err(fault) => {
            let prefix = message
                .as_deref()
                .map(|m| format!("{m}: "))
                .unwrap_or_default();
            let _ = writeln!(stderr, "save-output: {prefix}{}", fault.explain());
            return Ok(Verdict::Hit);
        }
    };

    let path = normalised(Path::new(output));
    let Some(record) = record_path(&path) else {
        return unusable(
            stderr,
            &format!("save-output: cannot write {shown_output}: it names no file"),
        );
    };
    if let Err(error) = write_both(&[(path, &captured.document), (record, &captured.record)]) {
        return unusable(
            stderr,
            &format!(
                "save-output: cannot write {shown_output}: {}",
                write_failure(&error)
            ),
        );
    }
    let _ = writeln!(stdout, "{}", py_rstrip(&captured.result));
    if let Some(message) = &message {
        let _ = writeln!(stdout, "document from {message}");
    }
    let _ = writeln!(
        stdout,
        "saved {} characters to {shown_output}",
        captured.characters
    );
    Ok(Verdict::Clean)
}

/// Reports `message` on `stderr` and gives it as the run's [`Unusable`] outcome.
fn unusable(stderr: &mut impl Write, message: &str) -> Outcome {
    let _ = writeln!(stderr, "{message}");
    Err(Unusable::new(message))
}

/// The text of the file at `path`, or why it cannot be had, in [`io_phrase`]'s words.
fn read_text(path: &Path) -> Result<String, &'static str> {
    let bytes = fs::read(path).map_err(|error| io_phrase::read_phrase(error.kind()))?;
    String::from_utf8(bytes).map_err(|_| io_phrase::NOT_UTF8)
}

/// Why OUTPUT or its record cannot be written, in [`io_phrase`]'s words.
fn write_failure(error: &io::Error) -> &'static str {
    io_phrase::write_phrase(error.kind())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(dir)
            .expect("the directory is readable")
            .map(|entry| {
                entry
                    .expect("an entry is readable")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        found.sort();
        found
    }

    /// Runs the tool on `operands` and gives its exit code, stdout and stderr.
    fn call(operands: &[&str]) -> (u8, String, String) {
        let operands: Vec<OsString> = operands.iter().map(OsString::from).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let outcome = run(&operands, &mut out, &mut err);
        (
            mintswarm_rules::exit::code(&outcome),
            String::from_utf8(out).expect("stdout is UTF-8"),
            String::from_utf8(err).expect("stderr is UTF-8"),
        )
    }

    #[test]
    fn record_path_replaces_or_appends_the_suffix() {
        let cases = [
            ("out/review.md", "out/review.result.json"),
            ("review.md", "review.result.json"),
            ("out/review", "out/review.result.json"),
            ("out/review.txt", "out/review.txt.result.json"),
            ("out/.md", "out/.result.json"),
            ("out/a.b.md", "out/a.b.result.json"),
            ("out/review.md/", "out/review.result.json"),
        ];
        for (output, record) in cases {
            assert_eq!(
                record_path(Path::new(output)),
                Some(PathBuf::from(record)),
                "{output}"
            );
        }
        assert_eq!(record_path(Path::new("/")), None);
        assert_eq!(record_path(Path::new("..")), None);
    }

    #[test]
    fn output_is_normalised_as_pathlib_does() {
        let cases = [
            ("out/review.md", "out/review.md"),
            ("out/review.md/", "out/review.md"),
            ("out//review.md", "out/review.md"),
            ("out/./review.md", "out/review.md"),
            ("out/.", "out"),
            ("/abs/review.md/", "/abs/review.md"),
            ("out/../review.md", "out/../review.md"),
        ];
        for (output, path) in cases {
            assert_eq!(
                normalised(Path::new(output)),
                PathBuf::from(path),
                "{output}"
            );
        }
    }

    #[test]
    fn operands_follow_save_output_py() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let missing = dir.path().join("missing");
        let missing = missing.to_str().expect("a UTF-8 path");
        let out = dir.path().join("review.md");
        let out = out.to_str().expect("a UTF-8 path");
        let usage = format!("{USAGE}\n");
        let rejected: &[&[&str]] = &[
            &[],
            &["--text"],
            &[missing],
            &["--text", missing],
            &[missing, out, "third"],
            &["--text", missing, out, "third"],
            &[missing, "--text", out],
            &[missing, out, "--text"],
            &["--", missing, out],
            &["--text", "--", out],
            &["--x", out],
            &[missing, "--x"],
            &["--text", "--text", out],
        ];
        for operands in rejected {
            let (code, stdout, stderr) = call(operands);
            assert_eq!(
                (code, stdout.as_str(), stderr.as_str()),
                (2, "", usage.as_str()),
                "{operands:?}"
            );
        }
        // accepted operands get as far as reading INPUT
        let accepted: &[&[&str]] = &[&[missing, out], &["--text", missing, out], &["-x", out]];
        for operands in accepted {
            let (code, _, stderr) = call(operands);
            assert_eq!(code, 2, "{operands:?}");
            assert!(
                stderr.starts_with("save-output: cannot read "),
                "{operands:?}: {stderr}"
            );
        }
        assert_eq!(names(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn write_both_writes_both_files() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let doc = dir.path().join("new/review.md");
        let record = dir.path().join("new/review.result.json");
        fs::create_dir(dir.path().join("new")).expect("a directory");
        fs::write(&doc, "stale\n").expect("a stale document");
        write_both(&[(doc.clone(), "# Doc\n"), (record.clone(), "{}\n")])
            .expect("both files are written");
        assert_eq!(fs::read_to_string(&doc).expect("the document"), "# Doc\n");
        assert_eq!(fs::read_to_string(&record).expect("the record"), "{}\n");
        assert_eq!(
            names(&dir.path().join("new")),
            ["review.md", "review.result.json"]
        );
    }

    #[test]
    fn write_both_creates_missing_parents() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let doc = dir.path().join("a/b/review.md");
        write_both(&[(doc.clone(), "# Doc\n")]).expect("the document is written");
        assert_eq!(fs::read_to_string(&doc).expect("the document"), "# Doc\n");
    }

    /// The Rust mirror of `save-output-selftest.py`'s case of the same name: the second rename
    /// fails after the first has put its file in place, and neither file nor any temporary is
    /// left.
    #[test]
    fn write_both_cleans_up_on_any_exception() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let doc = dir.path().join("review.md");
        let record = dir.path().join("review.result.json");
        fs::create_dir(&record).expect("a directory where the record goes");
        fs::write(record.join("keep"), "").expect("a file that keeps the directory non-empty");
        let written = write_both(&[(doc, "# Doc\n"), (record, "{}\n")]);
        assert!(written.is_err(), "the record cannot replace a directory");
        assert_eq!(names(dir.path()), ["review.result.json"]);
        assert_eq!(names(&dir.path().join("review.result.json")), ["keep"]);
    }

    #[test]
    fn write_both_leaves_nothing_when_a_parent_is_a_file() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        fs::write(dir.path().join("blocker"), "").expect("a regular file");
        let doc = dir.path().join("blocker/review.md");
        let record = dir.path().join("blocker/review.result.json");
        assert!(write_both(&[(doc, "# Doc\n"), (record, "{}\n")]).is_err());
        assert_eq!(names(dir.path()), ["blocker"]);
    }

    #[test]
    fn write_both_removes_the_first_temporary_when_the_second_cannot_start() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        fs::write(dir.path().join("blocker"), "").expect("a regular file");
        let doc = dir.path().join("review.md");
        let record = dir.path().join("blocker/review.result.json");
        assert!(write_both(&[(doc, "# Doc\n"), (record, "{}\n")]).is_err());
        assert_eq!(names(dir.path()), ["blocker"]);
    }
}
