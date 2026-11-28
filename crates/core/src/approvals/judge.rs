//! The approval judge, design #548 §6.3: every command comment of an issue, judged in one pass.
//!
//! [`judge`] is pure: the caller reads the issue, its comments, its owner and that owner's
//! role, and its `msw-state` value, and passes them in [`Facts`]; nothing here reads or
//! writes. Each `approve` or `reject` a person posted is judged by the nine ordered checks of
//! §6.3, and the first that fails is its [`Refusal`]; one that passes every check is bound to
//! the current approval package's kind, URL, comment id and sha256. A `help` or a near miss
//! gets its answer's context.
//!
//! The plane's own comments, those by one of [`Facts::self_logins`], are where packages,
//! records and answers come from. One that opens with a package or record marker is never a
//! command; any other that holds a command line is judged as a bot's. A command one of them
//! already answers gives no outcome, so a delivery seen twice is answered once.
//!
//! Comments are taken in `(created_at, id)` order. "Predates" and "edited" compare the RFC 3339
//! strings as the tracker gives them.

use std::collections::HashSet;
use std::fmt;

use mintswarm_config::{AWAITING_APPROVAL, Action, ActionTable, People, Trust};
use mintswarm_domain::tracker::{Comment, Issue, Login, RepoRole, is_bot};
use mintswarm_rules::approver::{NotApprover, issue_approver};
use mintswarm_rules::command::{self, Command, Hint, PackageKind, PackageRead, Reading};
use mintswarm_rules::render::decision::{self, Marker};

/// Who approves the issue, as the caller read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approver<'a> {
    /// The owner was not read, because [`needs_approver`] said no command needs it.
    NotRead,
    /// The parent chain gives no owner.
    NoOwner,
    /// The issue's owner and the owner's role on the repository.
    Owner { login: &'a Login, role: RepoRole },
}

/// What the judge is given: the caller's fresh reads and the repository's settings.
#[derive(Clone, Copy, Debug)]
pub struct Facts<'a> {
    /// The issue, as re-read.
    pub issue: &'a Issue,
    /// All its comments, in any order.
    pub comments: &'a [Comment],
    /// `tracker.ingest`, for check 2.
    pub trust: &'a Trust,
    /// The owner and role, for checks 3 and 4.
    pub approver: Approver<'a>,
    /// The `msw-state` value, `none` when unset.
    pub state: &'a str,
    /// `tracker.actions`, whose row for the package's kind check 3 reads.
    pub actions: &'a ActionTable,
    /// The plane's own logins, whose comments are its packages, records and answers.
    pub self_logins: &'a [Login],
}

/// The latest approval package on an issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package<'a> {
    /// The package's comment.
    pub comment: &'a Comment,
    /// What it asks to approve.
    pub kind: PackageKind,
    /// The digest its second line binds; `None` when the package is not current.
    pub sha256: Option<String>,
}

impl Package<'_> {
    /// What a command passing every check on this package binds; `None` when the package is
    /// not current.
    pub fn binding(&self) -> Option<Binding> {
        Some(Binding {
            kind: self.kind,
            package_url: self.comment.url.clone(),
            package_id: self.comment.id,
            sha256: self.sha256.clone()?,
        })
    }
}

/// What a passing command binds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The package's kind, the decision's context.
    pub kind: PackageKind,
    /// The package comment's URL.
    pub package_url: String,
    /// The package comment's id.
    pub package_id: u64,
    /// The digest the package's second line binds.
    pub sha256: String,
}

/// Why a command is not carried out, one class per failing check of §6.3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Check 1: the sender is a bot, the plane's own logins included.
    Bot,
    /// Check 2: the issue is not trusted.
    Untrusted,
    /// Check 3: the action's row in `tracker.actions` does not trust the issue's owner.
    NotTrustedWith(Action),
    /// Check 3: the issue's parent chain gives no owner.
    NoOwner,
    /// Check 3: the sender is not the issue's owner.
    NotOwner,
    /// Check 4: the sender, the owner, has this role, which cannot write.
    NoWriteAccess(RepoRole),
    /// Check 5: the issue is not awaiting approval; its `msw-state` value.
    NotAwaiting(String),
    /// Check 6: the issue has no current approval package.
    NoPackage,
    /// Check 7: the command is not after the current package.
    Predates,
    /// Check 8: the command was edited after it was posted.
    Edited,
    /// Check 9: the current package is already decided.
    AlreadyDecided,
}

impl Refusal {
    /// The number of the failing check, 1 to 9.
    pub fn check(&self) -> u8 {
        match self {
            Self::Bot => 1,
            Self::Untrusted => 2,
            Self::NotTrustedWith(_) | Self::NoOwner | Self::NotOwner => 3,
            Self::NoWriteAccess(_) => 4,
            Self::NotAwaiting(_) => 5,
            Self::NoPackage => 6,
            Self::Predates => 7,
            Self::Edited => 8,
            Self::AlreadyDecided => 9,
        }
    }

    /// The reason, as a refusal's answer gives it.
    pub fn reason(&self) -> String {
        match self {
            Self::Bot => "the sender is a bot".to_owned(),
            Self::Untrusted => "the issue is not trusted".to_owned(),
            Self::NotTrustedWith(action) => format!(
                "`tracker.actions.{}` does not trust the issue's owner",
                action.as_str()
            ),
            Self::NoOwner => "the issue has no owner who filed it or an ancestor".to_owned(),
            Self::NotOwner => "the sender is not the issue's owner".to_owned(),
            Self::NoWriteAccess(_) => "the sender has no write access to the repository".to_owned(),
            Self::NotAwaiting(state) => {
                format!("the issue is not awaiting approval (msw-state {state})")
            }
            Self::NoPackage => "the issue has no current approval package".to_owned(),
            Self::Predates => "it predates the current approval package".to_owned(),
            Self::Edited => "it was edited after it was posted".to_owned(),
            Self::AlreadyDecided => "the current approval package is already decided".to_owned(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason())
    }
}

/// What the judge makes of one comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// An `approve` that passed every check.
    Approve {
        binding: Binding,
        /// The comment's other lines, as [`command::read`] gives them.
        note: String,
    },
    /// A `reject` that passed every check.
    Reject { binding: Binding, note: String },
    /// An `approve` or `reject` that failed a check.
    Refused(Refusal),
    /// A `help`: the package kind its answer lists the commands of, `None` when no approval
    /// is awaited.
    Help { context: Option<PackageKind> },
    /// A near miss, with the context as for [`Verdict::Help`].
    Hint {
        hint: Hint,
        context: Option<PackageKind>,
    },
}

/// One judged comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judged {
    /// The comment's id, which an answer names.
    pub comment: u64,
    /// Its author.
    pub author: Login,
    pub verdict: Verdict,
}

/// A command needed the approver, but the caller passed [`Approver::NotRead`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("an approve or reject needs the issue's owner, which was not read")]
pub struct ApproverNotRead;

/// The order comments are taken in.
fn order(comment: &Comment) -> (&str, u64) {
    (comment.created_at.as_str(), comment.id)
}

/// Whether `login` is one of `self_logins`, compared ASCII case-insensitively.
fn is_own(login: &Login, self_logins: &[Login]) -> bool {
    self_logins
        .iter()
        .any(|own| own.0.eq_ignore_ascii_case(&login.0))
}

/// Whether `comment`'s author is a bot: flagged so, with a `[bot]` login, or one of the
/// plane's own logins.
fn by_bot(comment: &Comment, self_logins: &[Login]) -> bool {
    comment.author_is_bot || is_bot(&comment.author.0, None) || is_own(&comment.author, self_logins)
}

/// Whether `body` opens with a package or record marker, which the plane writes.
fn is_plane_record(body: &str) -> bool {
    command::package(body).is_some() || decision::read(body).is_some()
}

/// The comments in judging order.
fn sorted(comments: &[Comment]) -> Vec<&Comment> {
    let mut sorted: Vec<&Comment> = comments.iter().collect();
    sorted.sort_by(|a, b| order(a).cmp(&order(b)));
    sorted
}

/// The latest approval package among `comments`: the latest comment by one of `self_logins`
/// that [`command::package`] reads, current or not.
pub fn latest_package<'a>(comments: &'a [Comment], self_logins: &[Login]) -> Option<Package<'a>> {
    let mut latest: Option<Package<'a>> = None;
    for comment in comments {
        if !is_own(&comment.author, self_logins) {
            continue;
        }
        let Some(read) = command::package(&comment.body) else {
            continue;
        };
        if latest
            .as_ref()
            .is_some_and(|known| order(known.comment) > order(comment))
        {
            continue;
        }
        let (kind, sha256) = match read {
            PackageRead::Current { kind, sha256 } => (kind, Some(sha256)),
            PackageRead::NotCurrent(kind) => (kind, None),
        };
        latest = Some(Package {
            comment,
            kind,
            sha256,
        });
    }
    latest
}

/// The commands of `comments` that are still to answer, in judging order, each with its reading
/// and whether its author is a bot.
fn open_commands<'a>(
    comments: &'a [Comment],
    self_logins: &[Login],
) -> Vec<(&'a Comment, Reading, bool)> {
    let sorted = sorted(comments);
    let answered: HashSet<u64> = sorted
        .iter()
        .filter(|comment| is_own(&comment.author, self_logins))
        .filter_map(|comment| decision::answer_of(&comment.body))
        .collect();
    sorted
        .into_iter()
        .filter(|comment| {
            let record = is_own(&comment.author, self_logins) && is_plane_record(&comment.body);
            !record && !answered.contains(&comment.id)
        })
        .filter_map(|comment| {
            let reading = command::read(&comment.body)?;
            Some((comment, reading, by_bot(comment, self_logins)))
        })
        .collect()
}

/// Whether any `approve` or `reject` by a person is still to answer: only then does
/// [`judge`] need the owner and role, so the caller reads them only then.
pub fn needs_approver(comments: &[Comment], self_logins: &[Login]) -> bool {
    open_commands(comments, self_logins)
        .iter()
        .any(|(_, reading, bot)| !bot && is_decision(reading))
}

/// Whether `reading` is an `approve` or a `reject`.
fn is_decision(reading: &Reading) -> bool {
    matches!(
        reading,
        Reading::Command {
            command: Command::Approve | Command::Reject,
            ..
        }
    )
}

/// The action an approval of `kind` is.
fn action_of(kind: PackageKind) -> Action {
    match kind {
        PackageKind::Plan => Action::ApprovePlan,
        PackageKind::Design => Action::ApproveDesign,
        PackageKind::Decomposition => Action::ApproveDecomposition,
    }
}

/// Every command comment of the issue still to answer, judged in `(created_at, id)` order.
///
/// A help or near miss from a bot gives no outcome. Fails only when an `approve` or `reject`
/// reaches check 3 while the approver is [`Approver::NotRead`].
pub fn judge(facts: &Facts<'_>) -> Result<Vec<Judged>, ApproverNotRead> {
    let package = latest_package(facts.comments, facts.self_logins);
    let awaiting = facts.state == AWAITING_APPROVAL;
    let context = package
        .as_ref()
        .map(|package| package.kind)
        .filter(|_| awaiting);
    let current = package.as_ref().and_then(|package| {
        let binding = package.binding()?;
        Some((package.comment, binding))
    });
    let recorded = current.as_ref().is_some_and(|(package, _)| {
        facts.comments.iter().any(|comment| {
            is_own(&comment.author, facts.self_logins)
                && matches!(decision::read(&comment.body), Some(Marker::Decision { .. }))
                && order(comment) > order(package)
        })
    });

    let mut decided = recorded;
    let mut judged = Vec::new();
    for (comment, reading, bot) in open_commands(facts.comments, facts.self_logins) {
        let verdict = match reading {
            Reading::Hint(hint) if !bot => Verdict::Hint { hint, context },
            Reading::Command {
                command: Command::Help,
                ..
            } if !bot => Verdict::Help { context },
            Reading::Hint(_)
            | Reading::Command {
                command: Command::Help,
                ..
            } => continue,
            Reading::Command { command, note } => {
                match checks(
                    facts,
                    comment,
                    bot,
                    package.as_ref(),
                    current.as_ref(),
                    decided,
                )? {
                    Err(refusal) => Verdict::Refused(refusal),
                    Ok(binding) if command == Command::Reject => {
                        decided = true;
                        Verdict::Reject { binding, note }
                    }
                    Ok(binding) => {
                        decided = true;
                        Verdict::Approve { binding, note }
                    }
                }
            }
        };
        judged.push(Judged {
            comment: comment.id,
            author: comment.author.clone(),
            verdict,
        });
    }
    Ok(judged)
}

/// The first of §6.3's checks `comment` fails, or, when it passes them all, what it binds.
fn checks(
    facts: &Facts<'_>,
    comment: &Comment,
    bot: bool,
    package: Option<&Package<'_>>,
    current: Option<&(&Comment, Binding)>,
    decided: bool,
) -> Result<Result<Binding, Refusal>, ApproverNotRead> {
    // Check 1.
    if bot {
        return Ok(Err(Refusal::Bot));
    }
    // Check 2.
    if !facts.trust.trusts(facts.issue) {
        return Ok(Err(Refusal::Untrusted));
    }
    // Checks 3 and 4: the context's row, then the owner rule; with no package, the rule alone.
    if let Some(package) = package {
        let action = action_of(package.kind);
        if facts.actions.get(action).people != People::IssueOwner {
            return Ok(Err(Refusal::NotTrustedWith(action)));
        }
    }
    match facts.approver {
        Approver::NotRead => return Err(ApproverNotRead),
        Approver::NoOwner => return Ok(Err(Refusal::NoOwner)),
        Approver::Owner { login, role } => {
            match issue_approver(&comment.author, bot, login, role) {
                Ok(()) => {}
                Err(NotApprover::Bot) => return Ok(Err(Refusal::Bot)),
                Err(NotApprover::NotOwner) => return Ok(Err(Refusal::NotOwner)),
                Err(NotApprover::NoWriteAccess(role)) => {
                    return Ok(Err(Refusal::NoWriteAccess(role)));
                }
            }
        }
    }
    // Check 5.
    if facts.state != AWAITING_APPROVAL {
        return Ok(Err(Refusal::NotAwaiting(facts.state.to_owned())));
    }
    // Check 6.
    let Some((package, binding)) = current else {
        return Ok(Err(Refusal::NoPackage));
    };
    // Check 7.
    if order(comment) <= order(package) {
        return Ok(Err(Refusal::Predates));
    }
    // Check 8.
    if comment.updated_at != comment.created_at {
        return Ok(Err(Refusal::Edited));
    }
    // Check 9.
    if decided {
        return Ok(Err(Refusal::AlreadyDecided));
    }
    Ok(Ok(binding.clone()))
}
