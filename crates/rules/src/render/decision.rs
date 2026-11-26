//! The records the plane posts when it judges a command or a merge, design #548 §7, and the
//! readers of their markers.
//!
//! - [`Marker`] and [`read`]: the opening marker lines of every record, the only copy of them,
//!   written by [`Marker::lines`] and read back by [`read`] and [`answer_of`];
//! - [`record`]: the approval and rejection record of a plan, design or decomposition;
//! - [`refusal`], [`help`], [`hint`] and [`pr_hint`]: the answers to a comment;
//! - [`set_back`]: the reply when a hand-set approval is set back to `awaiting approval`;
//! - [`merge_ready`] and [`merge_refused`]: the merge records on a pull request;
//! - [`shadow_line`] and [`decide_shadow`]: the maintainer's decision in a shadow record, in
//!   place of [`PENDING_DECISION`].
//!
//! User text (a note, a reason, a login, a check name) goes in a [`span`] or, for the note, in a
//! fence longer than any backtick run it holds, so it carries no live mention and no marker the
//! readers take. The readers look at the opening lines only, so text further down never counts.
//!
//! The bodies on a pull request ([`merge_ready`], [`merge_refused`] and [`pr_hint`]) never carry a
//! `<!-- mintswarm: steer -->` line: on a pull request that line withdraws an approval.

use mintswarm_domain::ids::RevId;
use mintswarm_domain::records::Decision;
use mintswarm_domain::tracker::{CheckState, Login, Review};

use super::{commands, span};
use crate::command::{Command, Hint, PackageKind};
use crate::markdown::py_strip;

/// The start of the shadow record's maintainer decision row, before its value.
macro_rules! decision_label {
    () => {
        "- **Maintainer decision:** "
    };
}

/// The shadow record's maintainer decision row while no decision is made: the line
/// [`decide_shadow`] replaces.
pub const PENDING_DECISION: &str = concat!(decision_label!(), "pending");

/// What opens a marker line, before its text.
const MARKER_OPEN: &str = "<!-- mintswarm: ";

/// What closes a marker line, after its text.
const MARKER_CLOSE: &str = " -->";

/// The second line of a decision record: it marks the record as a maintainer decision.
const STEER: &str = "<!-- mintswarm: steer -->";

/// The marker text of an answer, before the comment id.
const ANSWER: &str = "answer ";

/// The marker text of a set-back, before the package comment id.
const SET_BACK: &str = "set-back ";

/// The marker text of a merge-ready record.
const MERGE_READY: &str = "merge ready";

/// The marker text of a merge refusal, before the head.
const MERGE_REFUSED: &str = "merge refused ";

/// The fewest backticks of the note's fence.
const MIN_FENCE: usize = 3;

/// How a decision went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ruling {
    /// The package is approved.
    Approved,
    /// The package is rejected.
    Rejected,
}

impl Ruling {
    /// Every ruling.
    pub const ALL: [Ruling; 2] = [Ruling::Approved, Ruling::Rejected];

    /// The ruling as a word: `approved` or `rejected`, as the marker and the shadow line write it.
    pub const fn word(self) -> &'static str {
        match self {
            Ruling::Approved => "approved",
            Ruling::Rejected => "rejected",
        }
    }
}

/// The opening marker of a record.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Marker {
    /// An approval or rejection record: `<!-- mintswarm: <kind> <ruling> -->`, the steer line,
    /// and `<!-- mintswarm: answer <id> -->` when the record answers a command comment.
    Decision {
        /// What was decided.
        kind: PackageKind,
        /// How it was decided.
        ruling: Ruling,
        /// The command comment the record answers; `None` for a value set by hand.
        answer: Option<u64>,
    },
    /// An answer to a comment: `<!-- mintswarm: answer <comment id> -->`.
    Answer(u64),
    /// A set-back of a package: `<!-- mintswarm: set-back <package comment id> -->`.
    SetBack(u64),
    /// A merge-ready record: `<!-- mintswarm: merge ready -->`.
    MergeReady,
    /// A merge refusal at a head: `<!-- mintswarm: merge refused <head sha> -->`.
    MergeRefused(RevId),
}

/// One marker line with `text` inside, ending in LF.
fn marker_line(text: &str) -> String {
    format!("{MARKER_OPEN}{text}{MARKER_CLOSE}\n")
}

impl Marker {
    /// The opening lines, each ending in LF: three for a decision with an answer, two for one
    /// without, one for every other marker.
    pub fn lines(&self) -> String {
        match self {
            Marker::Decision {
                kind,
                ruling,
                answer,
            } => {
                let mut out = marker_line(&format!("{} {}", kind.noun(), ruling.word()));
                out.push_str(STEER);
                out.push('\n');
                if let Some(id) = answer {
                    out.push_str(&marker_line(&format!("{ANSWER}{id}")));
                }
                out
            }
            Marker::Answer(id) => marker_line(&format!("{ANSWER}{id}")),
            Marker::SetBack(id) => marker_line(&format!("{SET_BACK}{id}")),
            Marker::MergeReady => marker_line(MERGE_READY),
            Marker::MergeRefused(head) => marker_line(&format!("{MERGE_REFUSED}{}", head.0)),
        }
    }
}

/// The text inside a stripped marker line.
fn marker_text(line: &str) -> Option<&str> {
    line.strip_prefix(MARKER_OPEN)?.strip_suffix(MARKER_CLOSE)
}

/// A decimal id with no sign and no leading zero.
fn id_of(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|c| c.is_ascii_digit());
    let leading_zero = text.len() > 1 && text.starts_with('0');
    if digits && !leading_zero {
        text.parse().ok()
    } else {
        None
    }
}

/// A head of 40 or 64 lowercase hexadecimal characters.
fn head_of(text: &str) -> Option<RevId> {
    let hex = text
        .bytes()
        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
    (hex && matches!(text.len(), 40 | 64)).then(|| RevId(text.to_owned()))
}

/// The kind and ruling a decision marker's text names.
fn decision_of(text: &str) -> Option<(PackageKind, Ruling)> {
    PackageKind::ALL.into_iter().find_map(|kind| {
        let ruling = text.strip_prefix(kind.noun())?.strip_prefix(' ')?;
        Ruling::ALL
            .into_iter()
            .find(|r| r.word() == ruling)
            .map(|r| (kind, r))
    })
}

/// The marker `body` opens with, or nothing.
///
/// CRLF becomes LF and each line is stripped; only the opening lines count. A decision marker
/// must be followed by the steer line exactly, and its third line is its answer only when the
/// whole line is an answer marker. Ids are decimal with no sign and no leading zero; a refused
/// head is 40 or 64 lowercase hexadecimal characters. Anything else is no marker.
pub fn read(body: &str) -> Option<Marker> {
    let text = body.replace("\r\n", "\n");
    let mut lines = text.split('\n').map(py_strip);
    let first = marker_text(lines.next()?)?;
    if let Some((kind, ruling)) = decision_of(first) {
        if lines.next()? != STEER {
            return None;
        }
        let answer = lines
            .next()
            .and_then(marker_text)
            .and_then(|text| text.strip_prefix(ANSWER))
            .and_then(id_of);
        return Some(Marker::Decision {
            kind,
            ruling,
            answer,
        });
    }
    if first == MERGE_READY {
        return Some(Marker::MergeReady);
    }
    if let Some(id) = first.strip_prefix(ANSWER) {
        return id_of(id).map(Marker::Answer);
    }
    if let Some(id) = first.strip_prefix(SET_BACK) {
        return id_of(id).map(Marker::SetBack);
    }
    if let Some(head) = first.strip_prefix(MERGE_REFUSED) {
        return head_of(head).map(Marker::MergeRefused);
    }
    None
}

/// The comment `body` answers: the id of an answer, or of a decision record with an answer.
pub fn answer_of(body: &str) -> Option<u64> {
    match read(body)? {
        Marker::Answer(id)
        | Marker::Decision {
            answer: Some(id), ..
        } => Some(id),
        _ => None,
    }
}

/// What a decision record is made of.
#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    /// What was decided.
    pub kind: PackageKind,
    /// How it was decided.
    pub ruling: Ruling,
    /// The command comment the record answers; `None` for a value set by hand.
    pub answer: Option<u64>,
    /// Who decided.
    pub approver: &'a Login,
    /// When, in RFC 3339, as a comment's `created_at`.
    pub time: &'a str,
    /// The approval package's URL.
    pub package_url: &'a str,
    /// The digest the package binds, as 64 lowercase hexadecimal characters.
    pub sha256: &'a str,
    /// The note the command comment carries; blank for none.
    pub note: &'a str,
}

/// The longest run of backticks in `text`.
fn longest_backtick_run(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

/// The note as a `Note:` paragraph: `Note: none.` when blank, else `Note:`, a blank line and the
/// note, CRLF made LF, in a backtick fence one longer than its longest backtick run and at least
/// [`MIN_FENCE`] long.
fn note_block(note: &str) -> String {
    if py_strip(note).is_empty() {
        return "Note: none.\n".to_owned();
    }
    let note = note.replace("\r\n", "\n");
    let fence = "`".repeat((longest_backtick_run(&note) + 1).max(MIN_FENCE));
    let newline = if note.ends_with('\n') { "" } else { "\n" };
    format!("Note:\n\n{fence}\n{note}{newline}{fence}\n")
}

/// The approval or rejection record: the [`Marker::Decision`] lines, the ruling with the
/// approver and time, the package's URL and digest line, and the note.
pub fn record(record: &Record<'_>) -> String {
    let marker = Marker::Decision {
        kind: record.kind,
        ruling: record.ruling,
        answer: record.answer,
    };
    format!(
        "{}\n**{} {}** by {} at {}.\n\n- Package: {}\n- {}\n\n{}",
        marker.lines(),
        record.kind.label(),
        record.ruling.word(),
        span(&record.approver.0),
        span(record.time),
        record.package_url,
        record.kind.digest_line(record.sha256),
        note_block(record.note)
    )
}

/// The answer to a command that is not carried out: `Not done: ` and the reason.
pub fn refusal(answer: u64, reason: &str) -> String {
    format!(
        "{}\nNot done: {}\n",
        Marker::Answer(answer).lines(),
        span(reason)
    )
}

/// What a help answer says after its marker: the Commands section of `kind`, or, when the issue
/// awaits no approval, that no command applies now.
fn commands_or_none(kind: Option<PackageKind>) -> String {
    match kind {
        Some(kind) => commands(kind),
        None => "No approval is awaited on this issue, so no command applies now.\n".to_owned(),
    }
}

/// The answer to `help`: the Commands section of the approval the issue awaits, if any.
pub fn help(answer: u64, kind: Option<PackageKind>) -> String {
    format!(
        "{}\n{}",
        Marker::Answer(answer).lines(),
        commands_or_none(kind)
    )
}

/// `text` as a sentence: its first letter in upper case, and a full stop unless it ends in one
/// or a question mark.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    let mut out: String = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if !out.ends_with(['.', '?']) {
        out.push('.');
    }
    out
}

/// The answer to a near miss: the hint as a sentence, then what [`help`] writes for `kind`.
pub fn hint(answer: u64, hint: &Hint, kind: Option<PackageKind>) -> String {
    format!(
        "{}\n{}\n\n{}",
        Marker::Answer(answer).lines(),
        sentence(&hint.to_string()),
        commands_or_none(kind)
    )
}

/// The answer to an approve command on a pull request: a pull request is approved by a review.
pub fn pr_hint(answer: u64) -> String {
    format!(
        "{}\nA pull request is approved by a GitHub review of its head commit; an `{}` comment is \
         no approval.\n",
        Marker::Answer(answer).lines(),
        Command::Approve.word()
    )
}

/// The reply when a hand-set approval of the package in comment `package_comment_id` is set
/// back to `awaiting approval`, with the reason and how to approve.
pub fn set_back(package_comment_id: u64, reason: &str) -> String {
    format!(
        "{}\n`msw-state` went back to `awaiting approval`: {}.\n\nTo approve, comment `{}` on a \
         line of its own.\n",
        Marker::SetBack(package_comment_id).lines(),
        span(reason),
        Command::Approve.word()
    )
}

/// The merge-ready record at `head`: the head, one line per approving review and one per check.
pub fn merge_ready(head: &RevId, reviews: &[Review], checks: &[CheckState]) -> String {
    let mut out = Marker::MergeReady.lines();
    out.push_str(&format!("\nHead: `{}`\n", head.0));
    let mut rows: Vec<String> = reviews
        .iter()
        .map(|review| {
            format!(
                "- Approved by {} ([review]({}))",
                span(&review.author.0),
                review.url
            )
        })
        .collect();
    rows.extend(checks.iter().map(|check| {
        format!(
            "- Check {}: {}",
            span(&check.name),
            span(check.conclusion.as_deref().unwrap_or("pending"))
        )
    }));
    if !rows.is_empty() {
        out.push('\n');
        for row in rows {
            out.push_str(&row);
            out.push('\n');
        }
    }
    out
}

/// The merge refusal at `head`: one line per reason.
pub fn merge_refused(head: &RevId, reasons: &[&str]) -> String {
    let mut out = Marker::MergeRefused(head.clone()).lines();
    out.push_str(&format!("\nNot merged at head `{}`:\n\n", head.0));
    for reason in reasons {
        out.push_str(&format!("- {}\n", span(reason)));
    }
    out
}

/// Whether the maintainer's ruling agrees with the judged decision: an approval with `auto`, a
/// rejection with `human`.
pub fn agrees(ruling: Ruling, judged: Decision) -> bool {
    matches!(
        (ruling, judged),
        (Ruling::Approved, Decision::Auto) | (Ruling::Rejected, Decision::Human)
    )
}

/// The shadow record's maintainer decision row once decided, without a line end.
pub fn shadow_line(ruling: Ruling, judged: Decision, record_url: &str) -> String {
    format!(
        concat!(
            decision_label!(),
            "{} · agrees with judged_decision: {} · [record]({})"
        ),
        ruling.word(),
        if agrees(ruling, judged) { "yes" } else { "no" },
        record_url
    )
}

/// `shadow` with its one [`PENDING_DECISION`] line replaced by [`shadow_line`], a trailing `\r`
/// kept and every other byte unchanged; nothing when there is no such line or more than one.
pub fn decide_shadow(
    shadow: &str,
    ruling: Ruling,
    judged: Decision,
    record_url: &str,
) -> Option<String> {
    let pieces: Vec<&str> = shadow.split_inclusive('\n').collect();
    let content = |piece: &str| -> usize {
        let line = piece.strip_suffix('\n').unwrap_or(piece);
        line.strip_suffix('\r').unwrap_or(line).len()
    };
    let mut pending = pieces
        .iter()
        .enumerate()
        .filter(|(_, piece)| piece[..content(piece)] == *PENDING_DECISION);
    let (index, _) = pending.next()?;
    if pending.next().is_some() {
        return None;
    }
    let mut out = String::with_capacity(shadow.len());
    for (i, piece) in pieces.iter().enumerate() {
        if i == index {
            out.push_str(&shadow_line(ruling, judged, record_url));
            out.push_str(&piece[content(piece)..]);
        } else {
            out.push_str(piece);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn malformed_ids_are_no_marker() {
        for id in [
            "+1",
            "01",
            "1 2",
            "",
            "-1",
            "1a",
            " 1",
            "18446744073709551616",
        ] {
            assert_eq!(read(&marker_line(&format!("answer {id}"))), None, "{id:?}");
            assert_eq!(
                read(&marker_line(&format!("set-back {id}"))),
                None,
                "{id:?}"
            );
        }
        assert_eq!(
            read("<!-- mintswarm: answer 0 -->"),
            Some(Marker::Answer(0))
        );
    }

    #[test]
    fn a_short_or_upper_case_head_is_no_marker() {
        for head in [
            &HEAD[..39],
            &HEAD[..12],
            &HEAD.to_uppercase(),
            &format!("{HEAD}0"),
        ] {
            let line = marker_line(&format!("merge refused {head}"));
            assert_eq!(read(&line), None, "{head:?}");
        }
        assert_eq!(
            read(&marker_line(&format!("merge refused {HEAD}"))),
            Some(Marker::MergeRefused(RevId(HEAD.to_owned())))
        );
    }

    #[test]
    fn a_decision_needs_its_steer_line() {
        assert_eq!(read("<!-- mintswarm: plan approved -->\n"), None);
        assert_eq!(read("<!-- mintswarm: plan approved -->"), None);
        assert_eq!(
            read("<!-- mintswarm: plan approved -->\n<!-- mintswarm: answer 3 -->\n"),
            None
        );
        assert_eq!(
            read("<!-- mintswarm: plan approved -->\n<!-- mintswarm: steer -->"),
            Some(Marker::Decision {
                kind: PackageKind::Plan,
                ruling: Ruling::Approved,
                answer: None,
            })
        );
    }

    #[test]
    fn a_malformed_third_line_is_no_answer() {
        for third in [
            "<!-- mintswarm: answer 01 -->",
            "<!-- mintswarm: answer 3 --> and more",
            "answer 3",
        ] {
            let body = format!(
                "<!-- mintswarm: design rejected -->\n<!-- mintswarm: steer -->\n{third}\n"
            );
            assert_eq!(
                read(&body),
                Some(Marker::Decision {
                    kind: PackageKind::Design,
                    ruling: Ruling::Rejected,
                    answer: None,
                }),
                "{third:?}"
            );
            assert_eq!(answer_of(&body), None);
        }
    }

    #[test]
    fn unknown_markers_are_none() {
        for body in [
            "<!-- mintswarm: plan accepted -->\n<!-- mintswarm: steer -->",
            "<!-- mintswarm: plan approval package -->",
            "<!-- mintswarm: steer -->",
            "<!-- mintswarm: merge ready  -->",
            "text\n<!-- mintswarm: answer 1 -->",
            "",
        ] {
            assert_eq!(read(body), None, "{body:?}");
        }
    }

    #[test]
    fn the_fence_outruns_every_backtick_run() {
        assert_eq!(longest_backtick_run("a ``b`` ````` c"), 5);
        assert_eq!(longest_backtick_run("none"), 0);
        assert!(note_block("x").contains("\n```\nx\n```\n"));
        assert!(note_block("a ````b").contains("\n`````\na ````b\n`````\n"));
        assert_eq!(note_block(" \r\n\t"), "Note: none.\n");
    }

    #[test]
    fn sentences_start_upper_and_end_stopped() {
        assert_eq!(
            sentence("did you mean `approve`?"),
            "Did you mean `approve`?"
        );
        assert_eq!(sentence("`/x` is not a command"), "`/x` is not a command.");
    }
}
