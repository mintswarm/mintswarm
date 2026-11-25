//! The bodies the session posts around a plan approval, as the session's scripts render them:
//! the TL;DR comment with its check (step 1 of `.mintswarm/dogfood/plan-approval/README.md` and
//! rule 11 of `.mintswarm/stages/plan.md`), the shadow record with its Artifacts line, and the
//! approval package.
//!
//! The session's own prose, the size lines, the Package rows, the Commands section and every URL
//! are the caller's: this module fixes the order and the wording around them.
//!
//! The approval package follows the bodies the session posted rather than the README's list
//! ("The approval package"): its Confidence row gives the band, the confidence, the authority
//! triggers listed and the reason, but not the threshold, the unusable hard triggers or the
//! doubts, and its Floor row names the hit rules without their reasons. The decision record and
//! the shadow record it links carry those.

use mintswarm_domain::records::{Decision, DecisionRecord, FloorRecord, LayerStatus, Verdict};

use super::comment::requires_maintainer;
use super::{bounded, code_list};
use crate::binding::digest;
use crate::markdown::py_strip;
use crate::plan_decision::envelope::band_name;
use crate::plan_floor::mode_name;

/// The first line of every approval package.
pub const PACKAGE_MARKER: &str = "<!-- mintswarm: plan approval package -->";

/// The fewest and the most bullets a usable TL;DR holds.
const TLDR_BULLETS: std::ops::RangeInclusive<usize> = 3..=6;

/// Why a plan's TL;DR is not usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TldrError {
    /// No `## TL;DR` line comes right after the title and summary line, before any other `## `
    /// heading.
    NotAfterSummary,
    /// Ignoring blank lines, the section does not hold three to six lines.
    Count,
    /// A line does not start with `- `.
    NotABullet,
    /// A line holds an odd number of backticks.
    UnpairedBacktick,
    /// A line holds `://` or `www.`.
    Url,
    /// Outside code spans, a line holds `@`, `<`, `>` or `](`.
    Markup,
    /// Outside code spans, a line holds a bare GitHub reference: `#` and a digit, or `GH-` and a
    /// digit.
    Reference,
    /// Outside code spans, a line holds a run of 40 hexadecimal characters.
    Sha,
}

impl std::fmt::Display for TldrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TldrError::NotAfterSummary => "no `## TL;DR` line follows the title and summary line",
            TldrError::Count => "the TL;DR does not hold three to six lines",
            TldrError::NotABullet => "a TL;DR line is not a `- ` bullet",
            TldrError::UnpairedBacktick => "a TL;DR line holds an unpaired backtick",
            TldrError::Url => "a TL;DR line holds a URL",
            TldrError::Markup => "a TL;DR line holds `@`, `<`, `>` or a link outside code spans",
            TldrError::Reference => "a TL;DR line holds a bare GitHub reference outside code spans",
            TldrError::Sha => "a TL;DR line holds a commit SHA outside code spans",
        })
    }
}

impl std::error::Error for TldrError {}

/// A line without its inline code spans: each pair of backticks and what lies between them
/// removed.
fn outside_spans(line: &str) -> String {
    line.split('`').step_by(2).collect()
}

fn has_reference(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let digit_at = |i: usize| chars.get(i).is_some_and(char::is_ascii_digit);
    (0..chars.len()).any(|i| {
        (chars[i] == '#' && digit_at(i + 1)) || (text_at(&chars, i, "GH-") && digit_at(i + 3))
    })
}

fn text_at(chars: &[char], at: usize, s: &str) -> bool {
    s.chars()
        .enumerate()
        .all(|(k, c)| chars.get(at + k) == Some(&c))
}

fn has_sha(text: &str) -> bool {
    let mut run = 0;
    for c in text.chars() {
        run = if c.is_ascii_hexdigit() { run + 1 } else { 0 };
        if run >= 40 {
            return true;
        }
    }
    false
}

/// The TL;DR bullets of `plan`, the non-blank lines of its section as written, or why the
/// section is not usable.
///
/// The section is the lines after the plan's first `## ` line, which must be `## TL;DR` and
/// follow only the `# ` title and one summary line, up to the next line starting with `## `.
/// Ignoring blank lines it holds three to six lines, each a `- ` bullet with an even number of
/// backticks and no URL (`://` or `www.`); outside its code spans no line holds `@`, `<`, `>`,
/// `](`, a bare GitHub reference (`#N`, `owner/repo#N`, `GH-N`) or a run of 40 hexadecimal
/// characters. Nothing is dropped or repaired.
pub fn tldr(plan: &str) -> Result<Vec<&str>, TldrError> {
    let lines: Vec<&str> = plan.split('\n').collect();
    let heading = lines
        .iter()
        .position(|line| line.starts_with("## "))
        .ok_or(TldrError::NotAfterSummary)?;
    let before: Vec<&str> = lines[..heading]
        .iter()
        .copied()
        .filter(|line| !py_strip(line).is_empty())
        .collect();
    let opened = matches!(before.as_slice(), [title, summary]
        if title.starts_with("# ") && !summary.starts_with('#'));
    if !opened || lines[heading].trim_end() != "## TL;DR" {
        return Err(TldrError::NotAfterSummary);
    }
    let end = (heading + 1..lines.len())
        .find(|&i| lines[i].starts_with("## "))
        .unwrap_or(lines.len());
    let bullets: Vec<&str> = lines[heading + 1..end]
        .iter()
        .copied()
        .filter(|line| !py_strip(line).is_empty())
        .collect();
    if !TLDR_BULLETS.contains(&bullets.len()) {
        return Err(TldrError::Count);
    }
    for line in &bullets {
        if !line.starts_with("- ") {
            return Err(TldrError::NotABullet);
        }
        if line.matches('`').count() % 2 == 1 {
            return Err(TldrError::UnpairedBacktick);
        }
        if line.contains("://") || line.contains("www.") {
            return Err(TldrError::Url);
        }
        let outside = outside_spans(line);
        if ["@", "<", ">", "]("].iter().any(|s| outside.contains(s)) {
            return Err(TldrError::Markup);
        }
        if has_reference(&outside) {
            return Err(TldrError::Reference);
        }
        if has_sha(&outside) {
            return Err(TldrError::Sha);
        }
    }
    Ok(bullets)
}

/// The TL;DR comment for `plan`, published as the plan artifact at `plan_url`: its TL;DR
/// bullets unchanged, or a line saying it has none that is usable, and the link to the plan.
pub fn tldr_comment(plan: &str, plan_url: &str) -> String {
    let body = match tldr(plan) {
        Ok(bullets) => bullets.join("\n"),
        Err(_) => "The plan has no usable TL;DR.".to_owned(),
    };
    format!("### Plan TL;DR\n\n{body}\n\nFull plan: {plan_url}\n")
}

/// The full URLs of the artifacts a shadow record links.
#[derive(Clone, Copy, Debug)]
pub struct Artifacts<'a> {
    /// The plan artifact.
    pub plan: &'a str,
    /// The `Plan assessment` artifact.
    pub assessment: &'a str,
    /// The `Plan approval decision` artifact.
    pub decision: &'a str,
}

/// The shadow record as the session posts it: the comment the decision printed, a blank line and
/// the Artifacts line.
pub fn shadow_record(comment: &str, artifacts: &Artifacts<'_>) -> String {
    format!(
        "{comment}\nArtifacts: [Plan]({}), [Plan assessment]({}), [Plan approval decision]({}).\n",
        artifacts.plan, artifacts.assessment, artifacts.decision
    )
}

/// What an approval package is made of.
#[derive(Clone, Copy, Debug)]
pub struct Package<'a> {
    /// The bytes of the `plan.md` published as the plan artifact.
    pub plan: &'a [u8],
    /// The TL;DR comment's URL.
    pub tldr_url: &'a str,
    /// The plan artifact's URL.
    pub plan_url: &'a str,
    /// The session's note: the approach and footprint in brief and the open questions, in
    /// Markdown, each line ending in a newline; placed as it is.
    pub note: &'a str,
    /// The lines `size.py plan` printed, or the line saying the size could not be read; none
    /// when the session has no size.
    pub size: &'a [&'a str],
    /// The floor verdict the package reports.
    pub floor: &'a FloorRecord,
    /// The Package rows of the declared packages.
    pub packages: &'a [String],
    /// The decision record.
    pub decision: &'a DecisionRecord,
    /// The `Plan assessment` artifact's URL.
    pub assessment_url: &'a str,
    /// The decision artifact's URL.
    pub decision_url: &'a str,
    /// The shadow record's URL, in `shadow` mode.
    pub shadow_url: Option<&'a str>,
    /// The Commands section, when the session gives one: its lines, each ending in a newline.
    pub commands: Option<&'a str>,
}

fn decision_name(decision: Decision) -> &'static str {
    match decision {
        Decision::Auto => "auto",
        Decision::Human => "human",
    }
}

/// The assessment's reason as the package shows it: the bounded inert text, in a code span when
/// it holds what a TL;DR line may not hold outside one.
fn reason_text(reason: &str) -> String {
    let text = bounded(reason);
    let unsafe_text = text.contains("://")
        || text.contains("www.")
        || ["@", "<", ">", "]("].iter().any(|s| text.contains(s))
        || has_reference(&text)
        || has_sha(&text);
    if unsafe_text {
        format!("`{text}`")
    } else {
        text
    }
}

/// The approval package: the marker and `Plan sha256:` lines, the heading, the TL;DR and plan
/// links, the session's note, the size lines, the floor's hit rules, the Package rows, the band
/// and confidence with the authority triggers listed and the assessment's reason, the mode and decision with their artifacts, and
/// the Commands section.
pub fn approval_package(package: &Package<'_>) -> String {
    let mut out = format!(
        "{PACKAGE_MARKER}\nPlan sha256: {}\n\n## Approval package: plan\n\n- **[TL;DR]({})** · \
         **[Plan]({})**\n",
        digest(package.plan),
        package.tldr_url,
        package.plan_url
    );
    out.push_str(package.note);
    for line in package.size {
        out.push_str(line);
        out.push('\n');
    }
    let hits: Vec<&str> = package
        .floor
        .rules
        .iter()
        .filter(|r| r.result == Verdict::Hit)
        .map(|r| r.rule.as_str())
        .collect();
    let floor = if hits.is_empty() {
        "clean".to_owned()
    } else {
        code_list(hits)
    };
    out.push_str(&format!("- **Floor:** {floor}.\n"));
    for row in package.packages {
        out.push_str(row);
        out.push('\n');
    }
    let judgement = &package.decision.layers.judgement;
    let confidence = match (judgement.status, judgement.band, judgement.confidence) {
        (LayerStatus::Usable, Some(band), Some(confidence)) => format!(
            "- **Confidence: {} · {confidence}**{} ([Plan assessment]({})): {}",
            band_name(band),
            requires_maintainer(judgement),
            package.assessment_url,
            reason_text(judgement.reasons.first().map_or("", String::as_str))
        ),
        _ => format!(
            "- **Confidence: unusable** ([Plan assessment]({}))",
            package.assessment_url
        ),
    };
    out.push_str(&confidence);
    out.push('\n');
    let record = package.decision;
    let shadow = package
        .shadow_url
        .map_or_else(String::new, |url| format!(", [shadow record]({url})"));
    let judged = if record.judged_decision == record.decision {
        String::new()
    } else {
        format!(
            ", `judged_decision` **{}**",
            decision_name(record.judged_decision)
        )
    };
    out.push_str(&format!(
        "- **Mode `{}`:** decision **{}**{judged} ([decision]({}){shadow}).\n",
        mode_name(record.mode),
        decision_name(record.decision),
        package.decision_url
    ));
    if let Some(commands) = package.commands {
        out.push('\n');
        out.push_str(commands);
    }
    out
}
