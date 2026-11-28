//! Blocker notes on active issues, design §7.
//!
//! A note is keyed by the issue's open-blocker set: the open listed blockers and the count of
//! unlisted ones, which count as open (B17). A note is posted only when the latest keyed note of
//! the bot shows a different set, so a duplicate delivery, or the second delivery of one link,
//! posts nothing.

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{Comment, Comments, Dependency, Login};
use mintswarm_github::NOTE_MARKER;

use super::rule::{BlockerChange, BlockerChangeKind};
use crate::tracker::github::note_key;

/// The start of every blocker note's key line.
pub const KEY_PREFIX: &str = "<!-- mintswarm: blockers";

/// The key line of a note on an issue whose open listed blockers are `open_listed` and which
/// has `unreadable` unlisted ones, in `repository`.
///
/// Blockers are sorted by repository and number, written `#N` in `repository` and
/// `owner/name#N` otherwise, so the same set always spells the same key:
/// `<!-- mintswarm: blockers open=#12,#15 unreadable=0 -->`, with an empty `open=` when none.
pub fn key(open_listed: &[Dependency], unreadable: u64, repository: &str) -> String {
    format!(
        "{KEY_PREFIX} open={} unreadable={unreadable} -->",
        names(open_listed, repository).join(",")
    )
}

/// The blockers' names, sorted and without repeats.
fn names(blockers: &[Dependency], repository: &str) -> Vec<String> {
    let mut sorted: Vec<(String, u64)> = blockers
        .iter()
        .map(|blocker| (blocker.repository.to_ascii_lowercase(), blocker.number.0))
        .collect();
    sorted.sort();
    sorted.dedup();
    sorted
        .into_iter()
        .map(|(name, number)| name_of(&name, number, repository))
        .collect()
}

/// The name of blocker `number` in `blocker_repository`, as seen from `repository`: `#N` in the
/// same repository (compared ASCII case-insensitively), `owner/name#N` otherwise, lowercased.
pub fn name_of(blocker_repository: &str, number: u64, repository: &str) -> String {
    if blocker_repository.eq_ignore_ascii_case(repository) {
        format!("#{number}")
    } else {
        format!("{}#{number}", blocker_repository.to_ascii_lowercase())
    }
}

/// The key of `bot`'s last blocker note among `comments`, if any.
pub fn latest_key<'c>(comments: &'c [Comment], bot: &Login) -> Option<&'c str> {
    comments.iter().rev().find_map(|comment| {
        let marked = comment
            .body
            .split('\n')
            .any(|line| line.trim() == NOTE_MARKER);
        if !marked || !comment.author.0.eq_ignore_ascii_case(&bot.0) {
            return None;
        }
        note_key(&comment.body).filter(|key| key.starts_with(KEY_PREFIX))
    })
}

/// What a blocker note says.
#[derive(Clone, Copy, Debug)]
pub struct Report<'a> {
    /// The plane's repository, `owner/name`.
    pub repository: &'a str,
    /// The changes that triggered the evaluation.
    pub changes: &'a [BlockerChange],
    /// The open listed blockers.
    pub open: &'a [Dependency],
    /// The blockers the listing did not return.
    pub unreadable: u64,
    /// The issue's `msw-state` value, which the note leaves as it is.
    pub value: &'a str,
}

impl Report<'_> {
    /// The note's key line.
    pub fn key(&self) -> String {
        key(self.open, self.unreadable, self.repository)
    }

    /// The note's body: the note marker, the key line, then the changes, the open blockers and
    /// the unchanged value.
    pub fn body(&self) -> String {
        let changes: Vec<String> = self
            .changes
            .iter()
            .map(|change| {
                format!(
                    "{} {}",
                    name_of(&change.repository, change.blocker.0, self.repository),
                    verb(change.change)
                )
            })
            .collect();
        let mut open = names(self.open, self.repository);
        if self.unreadable > 0 {
            open.push(format!("{} not readable", self.unreadable));
        }
        let open = if open.is_empty() {
            "none".to_owned()
        } else {
            open.join(", ")
        };
        format!(
            "{NOTE_MARKER}\n{key}\n\nBlockers changed: {changes}.\n\nOpen blockers: {open}.\n\n\
             `msw-state` stays `{value}`.\n",
            key = self.key(),
            changes = changes.join(", "),
            value = self.value,
        )
    }
}

fn verb(change: BlockerChangeKind) -> &'static str {
    match change {
        BlockerChangeKind::Closed => "closed",
        BlockerChangeKind::Reopened => "reopened",
        BlockerChangeKind::Added => "added",
        BlockerChangeKind::Removed => "removed",
    }
}

/// Posts `report` on `issue` as `bot`'s note when its key differs from the latest blocker note's
/// among `comments`, or there is none; returns whether it posted.
pub async fn post_if_changed<T: Comments>(
    tracker: &T,
    issue: IssueNumber,
    bot: &Login,
    comments: &[Comment],
    report: &Report<'_>,
) -> Result<bool, T::Error> {
    if latest_key(comments, bot) == Some(report.key().as_str()) {
        return Ok(false);
    }
    tracker.post_comment(issue, report.body()).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use mintswarm_domain::ids::IssueNumber;
    use mintswarm_domain::tracker::{Comment, Dependency, Login};
    use mintswarm_github::NOTE_MARKER;

    use super::{key, latest_key};

    const REPO: &str = "mintswarm/mintswarm";

    fn blocker(repository: &str, number: u64) -> Dependency {
        Dependency {
            repository: repository.to_owned(),
            number: IssueNumber(number),
            open: true,
        }
    }

    fn comment(author: &str, body: &str) -> Comment {
        Comment {
            id: 1,
            author: Login(author.to_owned()),
            author_is_bot: false,
            body: body.to_owned(),
            created_at: String::new(),
            updated_at: String::new(),
            url: String::new(),
        }
    }

    #[test]
    fn the_key_is_sorted_and_names_other_repositories() {
        let open = [
            blocker(REPO, 15),
            blocker("other/repo", 2),
            blocker("MintSwarm/MintSwarm", 12),
            blocker(REPO, 15),
        ];
        assert_eq!(
            key(&open, 1, REPO),
            "<!-- mintswarm: blockers open=#12,#15,other/repo#2 unreadable=1 -->"
        );
        assert_eq!(
            key(&[], 0, REPO),
            "<!-- mintswarm: blockers open= unreadable=0 -->"
        );
    }

    #[test]
    fn the_latest_key_is_the_bots_last_blocker_note() {
        let old = format!("{NOTE_MARKER}\n{}\n", key(&[blocker(REPO, 1)], 0, REPO));
        let new = format!("{NOTE_MARKER}\n{}\n", key(&[], 0, REPO));
        let other = format!("{NOTE_MARKER}\n<!-- mintswarm: plan 3 -->\n");
        let forged = format!("{NOTE_MARKER}\n{}\n", key(&[blocker(REPO, 9)], 0, REPO));
        let comments = [
            comment("bee[bot]", &old),
            comment("bee[bot]", &new),
            comment("bee[bot]", &other),
            comment("someone", &forged),
        ];
        let bot = Login("bee[bot]".to_owned());
        assert_eq!(
            latest_key(&comments, &bot),
            Some("<!-- mintswarm: blockers open= unreadable=0 -->")
        );
        assert_eq!(latest_key(&comments[2..], &bot), None);
    }
}
