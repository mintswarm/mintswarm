//! An in-memory tracker for tests: issues with their comments, links, dependencies, role field
//! values and statuses, seeded by the test, with every accepted write recorded in order.
//!
//! [`FakeTracker`] implements the capability traits the GitHub tracker does ([`Issues`],
//! [`Links`], [`Fields`], [`Statuses`], [`Comments`], [`Reactions`], [`CommentEdits`],
//! [`Collaborators`], [`Reviews`], [`CodeOwners`] and [`Merges`]), and refuses what it does: an
//! option the role's map does not list, a status the repository does not have, a comment without
//! a note key, a CODEOWNERS read at a revision that is not a full commit id, and the operations
//! the GitHub tracker does not support yet. A test may also make it refuse a comment's reactions
//! and edits, or an artifact's updates, as GitHub refuses a change the installation may not make.
//! It does not model labels, so a labels-mode role never reads as a conflict. A test may
//! script a role's value to change between reads with [`FakeTracker::script_field_changes`], as
//! a session writing between the plane's read and write would.
//!
//! Pull requests carry their reviews, review threads, requested reviewers and changed files, and
//! commits their checks, statuses and files, all seeded by the test. Each pull request has a
//! current head, which [`FakeTracker::move_head`] moves, so a test can move it between a check
//! and a merge. A merge is answered from the pull request's state, or from a script the test
//! queued with [`FakeTracker::script_merge`], and every merge call is recorded with its token.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard};

use mintswarm_domain::field::{
    FieldError, FieldFinding, FieldRead, FieldRef, FieldSource, FieldValue,
};
use mintswarm_domain::hosting::{CODEOWNERS_PATHS, CodeOwnersFile};
use mintswarm_domain::ids::{IssueNumber, PrNumber, RevId};
use mintswarm_domain::tracker::{
    Artifact, ChangedFile, CheckRun, CheckSuite, CodeOwners, Collaborators, CombinedStatus,
    Comment, CommentEdits, Comments, Cursor, Dependency, Direction, Fields, Issue, IssueRead,
    IssueType, Issues, Link, LinkKind, Links, Login, MergeOutcome, Merged, Merges, ParentIssue,
    PullDetail, Reactions, RepoRole, RequestedReviewers, Review, ReviewComment, ReviewThread,
    Reviews, Statuses, SubItem, TrackerBase, is_bot,
};
use mintswarm_domain::vcs::RepoPath;
use mintswarm_github::{NOTE_MARKER, Token, is_commit_id};

use crate::tracker::github::note_key;

/// The time a new fake stamps on comments until [`FakeTracker::set_now`] changes it.
const START: &str = "2026-01-01T00:00:00Z";

/// A write the fake accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeWrite {
    /// [`Fields::set_field`]; `option` is `None` for a clear.
    SetField {
        issue: IssueNumber,
        field: FieldSource,
        option: Option<String>,
    },
    /// [`Statuses::set_status`].
    SetStatus { issue: IssueNumber, status: String },
    /// [`Comments::post_comment`].
    PostComment { issue: IssueNumber, body: String },
    /// [`Reactions::react_plus_one`], recorded on every call, a repeated one included.
    React { comment: u64 },
    /// [`CommentEdits::edit_comment`].
    EditComment { comment: u64, body: String },
    /// [`CommentEdits::update_artifact`].
    UpdateArtifact {
        issue: IssueNumber,
        number: u64,
        body: String,
    },
    /// [`Merges::merge`], recorded on every call whatever its outcome, with the token it was
    /// given. The token's `Debug` is redacted.
    Merge {
        pr: PrNumber,
        sha: RevId,
        title: String,
        token: Token,
    },
}

/// A scripted answer to the next merge of a pull request, in place of the one its state gives.
///
/// Each carries its effect on the pull request's state, so a read-back after the merge agrees
/// with its answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeMerge {
    /// The merge answers [`MergeOutcome::Merged`] with this merge commit, and the pull request
    /// is merged and closed with it.
    Merged(Merged),
    /// The merge answers [`MergeOutcome::HeadMoved`], and the pull request's head moves to
    /// `head`, as a push racing the merge would.
    HeadMoved { head: RevId },
    /// The merge answers [`MergeOutcome::NotMergeable`]; the pull request is left as it is.
    NotMergeable,
    /// The merge fails with [`FakeTrackerError::MergeLost`]: its fate is unknown, `landed` is
    /// what [`Merges::may_have_landed`] says of it, and the pull request is left as it is.
    Lost { landed: bool },
    /// The merge fails with [`FakeTrackerError::MergeLost`], may have landed, and did: the pull
    /// request is merged and closed with this merge commit.
    LostButMerged(Merged),
}

/// Why a fake tracker call failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FakeTrackerError {
    /// No issue of this number was seeded.
    #[error("no issue #{0}", issue.0)]
    NoSuchIssue { issue: IssueNumber },
    /// The test made the issue unreadable.
    #[error("issue #{0} cannot be read", issue.0)]
    Unreadable { issue: IssueNumber },
    /// The repository has no status of this name; nothing was written.
    #[error("the repository has no issue status {name:?}")]
    UnknownStatus { name: String },
    /// The comment carries no note key; nothing was written.
    #[error("the note's body does not carry its marker and key lines")]
    Unmarked,
    /// No seeded or posted comment has this id; nothing was written.
    #[error("no comment {comment}")]
    NoSuchComment { comment: u64 },
    /// No artifact of this number was seeded on the issue; nothing was written.
    #[error("no artifact {number} on issue #{0}", issue.0)]
    NoSuchArtifact { issue: IssueNumber, number: u64 },
    /// The test made the comment or artifact refuse changes; nothing was written.
    #[error("the change was refused")]
    Refused,
    /// No pull request of this number was seeded.
    #[error("no pull request #{0}", pr.0)]
    NoSuchPull { pr: PrNumber },
    /// The revision is not a full commit id; nothing was read.
    #[error("{rev:?} is not a full commit id", rev = rev.0)]
    InvalidRev { rev: RevId },
    /// A scripted merge whose fate is unknown; `landed` says whether it may have landed.
    #[error("the merge's fate is unknown")]
    MergeLost { landed: bool },
    /// The GitHub tracker does not support the operation yet, so neither does the fake.
    #[error("the fake tracker does not support {operation}")]
    Unsupported { operation: &'static str },
}

#[derive(Debug)]
struct State {
    issues: HashMap<IssueNumber, (Issue, Vec<Comment>)>,
    links: HashMap<IssueNumber, Vec<Link>>,
    blocked_by: HashMap<IssueNumber, Vec<Dependency>>,
    blocking: HashMap<IssueNumber, Vec<Dependency>>,
    fields: HashMap<(IssueNumber, FieldSource), String>,
    statuses: HashMap<IssueNumber, String>,
    known_statuses: Option<Vec<String>>,
    unreadable: HashSet<IssueNumber>,
    field_errors: HashMap<IssueNumber, FieldError>,
    /// The scripted value changes, applied one after each read of the field; `None` clears.
    field_changes: HashMap<(IssueNumber, FieldSource), VecDeque<Option<String>>>,
    /// The comments carrying the bot's `+1`.
    reacted: HashSet<u64>,
    artifacts: HashMap<(IssueNumber, u64), Artifact>,
    /// The logins' roles; an unset login is [`RepoRole::NoAccess`].
    roles: HashMap<Login, RepoRole>,
    refused_comments: HashSet<u64>,
    refused_artifacts: HashSet<(IssueNumber, u64)>,
    /// The pull requests; each one's `head` is its current head.
    pulls: HashMap<PrNumber, PullDetail>,
    reviews: HashMap<PrNumber, Vec<Review>>,
    review_comments: HashMap<PrNumber, Vec<ReviewComment>>,
    review_threads: HashMap<PrNumber, Vec<ReviewThread>>,
    requested_reviewers: HashMap<PrNumber, RequestedReviewers>,
    changed_files: HashMap<PrNumber, Vec<ChangedFile>>,
    check_runs: HashMap<RevId, Vec<CheckRun>>,
    check_suites: HashMap<RevId, Vec<CheckSuite>>,
    commit_statuses: HashMap<RevId, CombinedStatus>,
    /// The repository's file texts, by commit and path.
    files: HashMap<(RevId, String), String>,
    /// The scripted merges of each pull request, answered first to last.
    merges: HashMap<PrNumber, Vec<FakeMerge>>,
    now: String,
    next_comment: u64,
    writes: Vec<FakeWrite>,
    /// How many times [`Collaborators::read_permission`] was called.
    permission_reads: usize,
    /// How many times [`Collaborators::read_parent`] was called.
    parent_reads: usize,
}

impl State {
    /// The issue, unless it is unknown or unreadable.
    fn issue(&self, issue: &IssueNumber) -> Result<&(Issue, Vec<Comment>), FakeTrackerError> {
        if self.unreadable.contains(issue) {
            return Err(FakeTrackerError::Unreadable {
                issue: issue.clone(),
            });
        }
        self.issues
            .get(issue)
            .ok_or_else(|| FakeTrackerError::NoSuchIssue {
                issue: issue.clone(),
            })
    }

    /// The issue's field checks: it must be readable and seeded, and carry no injected error.
    fn field_check(&self, issue: &IssueNumber) -> Result<(), FieldError> {
        self.issue(issue)
            .map_err(|error| FieldError::Transport(error.to_string()))?;
        match self.field_errors.get(issue) {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    /// The comment `id` on any issue, unless it is unknown or refuses changes.
    fn comment_mut(&mut self, id: u64) -> Result<&mut Comment, FakeTrackerError> {
        let refused = self.refused_comments.contains(&id);
        let comment = self
            .issues
            .values_mut()
            .flat_map(|(_, comments)| comments.iter_mut())
            .find(|comment| comment.id == id)
            .ok_or(FakeTrackerError::NoSuchComment { comment: id })?;
        if refused {
            return Err(FakeTrackerError::Refused);
        }
        Ok(comment)
    }

    /// The pull request, unless it is unknown.
    fn pull(&self, pr: &PrNumber) -> Result<&PullDetail, FakeTrackerError> {
        self.pulls
            .get(pr)
            .ok_or_else(|| FakeTrackerError::NoSuchPull { pr: pr.clone() })
    }

    /// A seeded per-PR list, after checking that the pull request is known.
    fn pr_list<V: Clone>(
        &self,
        pr: &PrNumber,
        lists: &HashMap<PrNumber, Vec<V>>,
    ) -> Result<Vec<V>, FakeTrackerError> {
        self.pull(pr)?;
        Ok(lists.get(pr).cloned().unwrap_or_default())
    }

    /// The answer to a merge of `pr` at `sha`: its script if one is queued, and otherwise its
    /// state, not mergeable first, then head moved, else merged.
    fn merge(&mut self, pr: &PrNumber, sha: &RevId) -> Result<MergeOutcome, FakeTrackerError> {
        let script = self
            .merges
            .get_mut(pr)
            .filter(|queue| !queue.is_empty())
            .map(|queue| queue.remove(0));
        let pull = self
            .pulls
            .get_mut(pr)
            .ok_or_else(|| FakeTrackerError::NoSuchPull { pr: pr.clone() })?;
        match script {
            Some(FakeMerge::Merged(merged)) => {
                mark_merged(pull, &merged.sha);
                Ok(MergeOutcome::Merged(merged))
            }
            Some(FakeMerge::HeadMoved { head }) => {
                pull.head = head;
                Ok(MergeOutcome::HeadMoved)
            }
            Some(FakeMerge::NotMergeable) => Ok(MergeOutcome::NotMergeable),
            Some(FakeMerge::Lost { landed }) => Err(FakeTrackerError::MergeLost { landed }),
            Some(FakeMerge::LostButMerged(merged)) => {
                mark_merged(pull, &merged.sha);
                Err(FakeTrackerError::MergeLost { landed: true })
            }
            None => {
                if pull.merged || pull.state != "open" || pull.mergeable == Some(false) {
                    return Ok(MergeOutcome::NotMergeable);
                }
                if pull.head != *sha {
                    return Ok(MergeOutcome::HeadMoved);
                }
                let commit = RevId(format!("{:040x}", pr.0));
                mark_merged(pull, &commit);
                Ok(MergeOutcome::Merged(Merged { sha: commit }))
            }
        }
    }

    fn dependencies(&mut self, direction: Direction) -> &mut HashMap<IssueNumber, Vec<Dependency>> {
        match direction {
            Direction::BlockedBy => &mut self.blocked_by,
            Direction::Blocking => &mut self.blocking,
        }
    }
}

/// An in-memory tracker that posts as `bot` and records every accepted write.
#[derive(Debug)]
pub struct FakeTracker {
    bot: Login,
    state: Mutex<State>,
}

impl FakeTracker {
    /// An empty tracker whose comments are posted as `bot`, accepting any status.
    pub fn new(bot: Login) -> Self {
        Self {
            bot,
            state: Mutex::new(State {
                issues: HashMap::new(),
                links: HashMap::new(),
                blocked_by: HashMap::new(),
                blocking: HashMap::new(),
                fields: HashMap::new(),
                statuses: HashMap::new(),
                known_statuses: None,
                unreadable: HashSet::new(),
                field_errors: HashMap::new(),
                field_changes: HashMap::new(),
                reacted: HashSet::new(),
                artifacts: HashMap::new(),
                roles: HashMap::new(),
                refused_comments: HashSet::new(),
                refused_artifacts: HashSet::new(),
                pulls: HashMap::new(),
                reviews: HashMap::new(),
                review_comments: HashMap::new(),
                review_threads: HashMap::new(),
                requested_reviewers: HashMap::new(),
                changed_files: HashMap::new(),
                check_runs: HashMap::new(),
                check_suites: HashMap::new(),
                commit_statuses: HashMap::new(),
                files: HashMap::new(),
                merges: HashMap::new(),
                now: START.to_owned(),
                next_comment: 1,
                writes: Vec::new(),
                permission_reads: 0,
                parent_reads: 0,
            }),
        }
    }

    /// The tracker accepting only the statuses named in `names`.
    #[must_use]
    pub fn with_statuses<'a>(self, names: impl IntoIterator<Item = &'a str>) -> Self {
        self.state().known_statuses = Some(names.into_iter().map(str::to_owned).collect());
        self
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panicking test leaves nothing another test shares.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Seeds `issue`, replacing any issue of its number and keeping that issue's comments.
    pub fn add_issue(&self, issue: Issue) {
        let mut state = self.state();
        let comments = state
            .issues
            .remove(&issue.number)
            .map(|(_, comments)| comments)
            .unwrap_or_default();
        state.issues.insert(issue.number.clone(), (issue, comments));
    }

    /// Seeds a comment on a seeded issue, without recording a write.
    ///
    /// # Panics
    ///
    /// When `issue` was not seeded.
    pub fn add_comment(&self, issue: &IssueNumber, comment: Comment) {
        let mut state = self.state();
        let Some((_, comments)) = state.issues.get_mut(issue) else {
            panic!("no issue #{} was seeded", issue.0);
        };
        comments.push(comment);
    }

    /// Seeds `issue`'s links, replacing any.
    pub fn set_links(&self, issue: &IssueNumber, links: Vec<Link>) {
        self.state().links.insert(issue.clone(), links);
    }

    /// Seeds the issues that block `issue`, or that it blocks, replacing any.
    pub fn set_dependencies(
        &self,
        issue: &IssueNumber,
        direction: Direction,
        dependencies: Vec<Dependency>,
    ) {
        self.state()
            .dependencies(direction)
            .insert(issue.clone(), dependencies);
    }

    /// Seeds the raw option name a role's field holds on `issue`, mapped or not.
    pub fn put_field(&self, issue: &IssueNumber, field: FieldSource, option: &str) {
        self.state()
            .fields
            .insert((issue.clone(), field), option.to_owned());
    }

    /// Seeds `issue`'s status.
    pub fn put_status(&self, issue: &IssueNumber, status: &str) {
        self.state()
            .statuses
            .insert(issue.clone(), status.to_owned());
    }

    /// Makes every read of `issue` fail.
    pub fn unreadable(&self, issue: &IssueNumber) {
        self.state().unreadable.insert(issue.clone());
    }

    /// Makes every field read and write of `issue` fail with `error`.
    pub fn fail_fields(&self, issue: &IssueNumber, error: FieldError) {
        self.state().field_errors.insert(issue.clone(), error);
    }

    /// Queues `changes` to a role's field on `issue`: each later read of the field answers from
    /// the value it holds, then applies the next queued change, `None` clearing the field. The
    /// changes are not recorded as writes.
    pub fn script_field_changes(
        &self,
        issue: &IssueNumber,
        field: FieldSource,
        changes: Vec<Option<String>>,
    ) {
        self.state()
            .field_changes
            .entry((issue.clone(), field))
            .or_default()
            .extend(changes);
    }

    /// Seeds the artifact numbered `number` on `issue`, replacing any.
    pub fn add_artifact(&self, issue: &IssueNumber, number: u64, artifact: Artifact) {
        self.state()
            .artifacts
            .insert((issue.clone(), number), artifact);
    }

    /// Seeds `login`'s role on the repository.
    pub fn set_role(&self, login: Login, role: RepoRole) {
        self.state().roles.insert(login, role);
    }

    /// Makes every reaction to and edit of the comment `id` fail with
    /// [`FakeTrackerError::Refused`].
    pub fn refuse_comment(&self, id: u64) {
        self.state().refused_comments.insert(id);
    }

    /// Makes every update of the artifact numbered `number` on `issue` fail with
    /// [`FakeTrackerError::Refused`].
    pub fn refuse_artifact(&self, issue: &IssueNumber, number: u64) {
        self.state()
            .refused_artifacts
            .insert((issue.clone(), number));
    }

    /// Seeds `pull`, replacing any pull request of its number; its `head` becomes the pull
    /// request's current head.
    pub fn add_pull(&self, pull: PullDetail) {
        self.state().pulls.insert(pull.number.clone(), pull);
    }

    /// Seeds `pr`'s reviews, replacing any.
    pub fn set_reviews(&self, pr: &PrNumber, reviews: Vec<Review>) {
        self.state().reviews.insert(pr.clone(), reviews);
    }

    /// Seeds `pr`'s review comments, replacing any.
    pub fn set_review_comments(&self, pr: &PrNumber, comments: Vec<ReviewComment>) {
        self.state().review_comments.insert(pr.clone(), comments);
    }

    /// Seeds `pr`'s review threads, replacing any.
    pub fn set_review_threads(&self, pr: &PrNumber, threads: Vec<ReviewThread>) {
        self.state().review_threads.insert(pr.clone(), threads);
    }

    /// Seeds the reviewers `pr`'s review is still requested from, replacing any.
    pub fn set_requested_reviewers(&self, pr: &PrNumber, requested: RequestedReviewers) {
        self.state()
            .requested_reviewers
            .insert(pr.clone(), requested);
    }

    /// Seeds the files `pr` changes, replacing any.
    pub fn set_changed_files(&self, pr: &PrNumber, files: Vec<ChangedFile>) {
        self.state().changed_files.insert(pr.clone(), files);
    }

    /// Seeds the check runs on commit `sha`, replacing any.
    pub fn set_check_runs(&self, sha: &RevId, runs: Vec<CheckRun>) {
        self.state().check_runs.insert(sha.clone(), runs);
    }

    /// Seeds the check suites on commit `sha`, replacing any.
    pub fn set_check_suites(&self, sha: &RevId, suites: Vec<CheckSuite>) {
        self.state().check_suites.insert(sha.clone(), suites);
    }

    /// Seeds the combined statuses on commit `sha`, replacing any.
    pub fn set_statuses(&self, sha: &RevId, status: CombinedStatus) {
        self.state().commit_statuses.insert(sha.clone(), status);
    }

    /// Seeds the text of the file at `path` in commit `rev`, replacing any.
    pub fn put_file(&self, rev: &RevId, path: &str, text: &str) {
        self.state()
            .files
            .insert((rev.clone(), path.to_owned()), text.to_owned());
    }

    /// Moves `pr`'s current head to `head`, as a push to its branch would.
    ///
    /// # Panics
    ///
    /// When `pr` was not seeded.
    pub fn move_head(&self, pr: &PrNumber, head: RevId) {
        let mut state = self.state();
        let Some(pull) = state.pulls.get_mut(pr) else {
            panic!("no pull request #{} was seeded", pr.0);
        };
        pull.head = head;
    }

    /// Queues `merge` as the answer to `pr`'s next unscripted merge call; scripts are answered
    /// first to last.
    pub fn script_merge(&self, pr: &PrNumber, merge: FakeMerge) {
        self.state()
            .merges
            .entry(pr.clone())
            .or_default()
            .push(merge);
    }

    /// The pull request `pr` as seeded, moved or merged.
    pub fn pull(&self, pr: &PrNumber) -> Option<PullDetail> {
        self.state().pulls.get(pr).cloned()
    }

    /// The RFC 3339 time stamped on comments posted or edited from now on.
    pub fn set_now(&self, now: &str) {
        self.state().now = now.to_owned();
    }

    /// Every accepted write, in order.
    pub fn writes(&self) -> Vec<FakeWrite> {
        self.state().writes.clone()
    }

    /// How many times [`Collaborators::read_permission`] was called.
    pub fn permission_reads(&self) -> usize {
        self.state().permission_reads
    }

    /// How many times [`Collaborators::read_parent`] was called.
    pub fn parent_reads(&self) -> usize {
        self.state().parent_reads
    }

    /// The raw option name a role's field holds on `issue`.
    pub fn field(&self, issue: &IssueNumber, field: &FieldSource) -> Option<String> {
        self.state()
            .fields
            .get(&(issue.clone(), field.clone()))
            .cloned()
    }

    /// `issue`'s status.
    pub fn status(&self, issue: &IssueNumber) -> Option<String> {
        self.state().statuses.get(issue).cloned()
    }

    /// The artifact numbered `number` on `issue`, as seeded or last updated.
    pub fn artifact(&self, issue: &IssueNumber, number: u64) -> Option<Artifact> {
        self.state()
            .artifacts
            .get(&(issue.clone(), number))
            .cloned()
    }

    /// Whether the comment `id` carries the bot's `+1`.
    pub fn reacted(&self, id: u64) -> bool {
        self.state().reacted.contains(&id)
    }

    /// `issue`'s comments, seeded and posted, in order.
    pub fn comments(&self, issue: &IssueNumber) -> Vec<Comment> {
        self.state()
            .issues
            .get(issue)
            .map(|(_, comments)| comments.clone())
            .unwrap_or_default()
    }
}

impl TrackerBase for FakeTracker {
    type Error = FakeTrackerError;
}

impl Issues for FakeTracker {
    async fn fetch_ready(&self) -> Result<Vec<Issue>, Self::Error> {
        Err(FakeTrackerError::Unsupported {
            operation: "fetch_ready",
        })
    }

    /// The issue and its comments updated at or after `since`; the cursor is the latest
    /// `updated_at` among them, else `since`, else the fake's current time.
    async fn read_issue(
        &self,
        issue: IssueNumber,
        since: Option<Cursor>,
    ) -> Result<IssueRead, Self::Error> {
        let state = self.state();
        let (read, comments) = state.issue(&issue)?;
        let comments: Vec<Comment> = comments
            .iter()
            .filter(|comment| {
                since
                    .as_ref()
                    .is_none_or(|Cursor(since)| comment.updated_at.as_str() >= since.as_str())
            })
            .cloned()
            .collect();
        let cursor = comments
            .iter()
            .map(|comment| comment.updated_at.as_str())
            .max()
            .map(|latest| Cursor(latest.to_owned()))
            .or(since)
            .unwrap_or_else(|| Cursor(state.now.clone()));
        Ok(IssueRead {
            issue: read.clone(),
            comments,
            cursor,
        })
    }

    async fn create_sub_item(
        &self,
        _parent: IssueNumber,
        _item: SubItem,
    ) -> Result<Issue, Self::Error> {
        Err(FakeTrackerError::Unsupported {
            operation: "create_sub_item",
        })
    }

    async fn set_issue_type(
        &self,
        _issue: IssueNumber,
        _kind: IssueType,
    ) -> Result<(), Self::Error> {
        Err(FakeTrackerError::Unsupported {
            operation: "set_issue_type",
        })
    }
}

impl Links for FakeTracker {
    async fn read_links(&self, issue: IssueNumber) -> Result<Vec<Link>, Self::Error> {
        let state = self.state();
        state.issue(&issue)?;
        Ok(state.links.get(&issue).cloned().unwrap_or_default())
    }

    async fn link_dependency(
        &self,
        _issue: IssueNumber,
        _blocker: IssueNumber,
    ) -> Result<(), Self::Error> {
        Err(FakeTrackerError::Unsupported {
            operation: "link_dependency",
        })
    }

    async fn read_dependencies(
        &self,
        issue: IssueNumber,
        direction: Direction,
    ) -> Result<Vec<Dependency>, Self::Error> {
        let mut state = self.state();
        state.issue(&issue)?;
        Ok(state
            .dependencies(direction)
            .get(&issue)
            .cloned()
            .unwrap_or_default())
    }
}

impl Statuses for FakeTracker {
    async fn read_status(&self, issue: IssueNumber) -> Result<Option<String>, Self::Error> {
        let state = self.state();
        state.issue(&issue)?;
        Ok(state.statuses.get(&issue).cloned())
    }

    async fn set_status(&self, issue: IssueNumber, status: String) -> Result<(), Self::Error> {
        let mut state = self.state();
        state.issue(&issue)?;
        if let Some(known) = &state.known_statuses
            && !known.contains(&status)
        {
            return Err(FakeTrackerError::UnknownStatus { name: status });
        }
        state.statuses.insert(issue.clone(), status.clone());
        state.writes.push(FakeWrite::SetStatus { issue, status });
        Ok(())
    }
}

impl Comments for FakeTracker {
    /// Posts `body` as the bot's note; it must hold the note marker line and a key line, as the
    /// GitHub tracker requires.
    async fn post_comment(&self, issue: IssueNumber, body: String) -> Result<Comment, Self::Error> {
        let marked = body.split('\n').any(|line| line.trim() == NOTE_MARKER);
        if !marked || note_key(&body).is_none() {
            return Err(FakeTrackerError::Unmarked);
        }
        let mut state = self.state();
        state.issue(&issue)?;
        let id = state.next_comment;
        state.next_comment += 1;
        let comment = Comment {
            id,
            author: self.bot.clone(),
            author_is_bot: is_bot(&self.bot.0, None),
            body: body.clone(),
            created_at: state.now.clone(),
            updated_at: state.now.clone(),
            url: format!("fake://issues/{}#comment-{id}", issue.0),
        };
        if let Some((_, comments)) = state.issues.get_mut(&issue) {
            comments.push(comment.clone());
        }
        state.writes.push(FakeWrite::PostComment { issue, body });
        Ok(comment)
    }
}

impl Reactions for FakeTracker {
    /// Records the reaction on every call; the comment keeps one `+1` however often it is made.
    async fn react_plus_one(&self, comment_id: u64) -> Result<(), Self::Error> {
        let mut state = self.state();
        state.comment_mut(comment_id)?;
        state.reacted.insert(comment_id);
        state.writes.push(FakeWrite::React {
            comment: comment_id,
        });
        Ok(())
    }
}

impl CommentEdits for FakeTracker {
    /// Sets the comment's body and stamps its `updated_at` with the fake's current time.
    async fn edit_comment(&self, comment_id: u64, body: String) -> Result<Comment, Self::Error> {
        let mut state = self.state();
        let now = state.now.clone();
        let comment = state.comment_mut(comment_id)?;
        comment.body.clone_from(&body);
        comment.updated_at = now;
        let edited = comment.clone();
        state.writes.push(FakeWrite::EditComment {
            comment: comment_id,
            body,
        });
        Ok(edited)
    }

    async fn update_artifact(
        &self,
        issue: IssueNumber,
        number: u64,
        body: String,
    ) -> Result<Artifact, Self::Error> {
        let mut state = self.state();
        let key = (issue.clone(), number);
        let refused = state.refused_artifacts.contains(&key);
        let Some(artifact) = state.artifacts.get_mut(&key) else {
            return Err(FakeTrackerError::NoSuchArtifact { issue, number });
        };
        if refused {
            return Err(FakeTrackerError::Refused);
        }
        artifact.body.clone_from(&body);
        let updated = artifact.clone();
        state.writes.push(FakeWrite::UpdateArtifact {
            issue,
            number,
            body,
        });
        Ok(updated)
    }
}

impl Collaborators for FakeTracker {
    async fn read_permission(&self, login: Login) -> Result<RepoRole, Self::Error> {
        let mut state = self.state();
        state.permission_reads += 1;
        Ok(state
            .roles
            .get(&login)
            .copied()
            .unwrap_or(RepoRole::NoAccess))
    }

    /// The issue's seeded [`LinkKind::Parent`] link, so it agrees with [`Links::read_links`],
    /// with the seeded parent issue's author and bot flag. A parent link to an issue that is
    /// not seeded is [`FakeTrackerError::NoSuchIssue`].
    async fn read_parent(&self, issue: IssueNumber) -> Result<Option<ParentIssue>, Self::Error> {
        let mut state = self.state();
        state.parent_reads += 1;
        state.issue(&issue)?;
        let Some(number) = state.links.get(&issue).and_then(|links| {
            links
                .iter()
                .find(|link| link.kind == LinkKind::Parent)
                .map(|link| IssueNumber(link.number))
        }) else {
            return Ok(None);
        };
        let (parent, _) = state.issue(&number)?;
        Ok(Some(ParentIssue {
            number,
            author: Some(parent.author.clone()),
            author_is_bot: parent.author_is_bot,
        }))
    }
}

impl Fields for FakeTracker {
    async fn read_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
    ) -> Result<FieldRead, FieldError> {
        let mut state = self.state();
        state.field_check(&issue)?;
        let key = (issue, field.source.clone());
        let read = match state.fields.get(&key) {
            None => Ok(FieldRead::Unset),
            Some(name) if !field.options.contains(name) => {
                Err(FieldError::Field(FieldFinding::OptionUnmapped))
            }
            Some(name) => Ok(FieldRead::Value(FieldValue {
                field_id: None,
                option_id: None,
                name: name.clone(),
            })),
        };
        if let Some(change) = state
            .field_changes
            .get_mut(&key)
            .and_then(VecDeque::pop_front)
        {
            match change {
                Some(name) => state.fields.insert(key, name),
                None => state.fields.remove(&key),
            };
        }
        read
    }

    async fn set_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
        option: Option<&str>,
    ) -> Result<(), FieldError> {
        let mut state = self.state();
        state.field_check(&issue)?;
        if let Some(name) = option
            && !field.options.iter().any(|known| known == name)
        {
            return Err(FieldError::Field(FieldFinding::OptionUnmapped));
        }
        let key = (issue.clone(), field.source.clone());
        match option {
            Some(name) => state.fields.insert(key, name.to_owned()),
            None => state.fields.remove(&key),
        };
        state.writes.push(FakeWrite::SetField {
            issue,
            field: field.source.clone(),
            option: option.map(str::to_owned),
        });
        Ok(())
    }
}

impl Reviews for FakeTracker {
    async fn read_pull(&self, pr: PrNumber) -> Result<PullDetail, Self::Error> {
        self.state().pull(&pr).cloned()
    }

    async fn read_reviews(&self, pr: PrNumber) -> Result<Vec<Review>, Self::Error> {
        let state = self.state();
        state.pr_list(&pr, &state.reviews)
    }

    async fn read_review_comments(&self, pr: PrNumber) -> Result<Vec<ReviewComment>, Self::Error> {
        let state = self.state();
        state.pr_list(&pr, &state.review_comments)
    }

    async fn read_review_threads(&self, pr: PrNumber) -> Result<Vec<ReviewThread>, Self::Error> {
        let state = self.state();
        state.pr_list(&pr, &state.review_threads)
    }

    async fn read_requested_reviewers(
        &self,
        pr: PrNumber,
    ) -> Result<RequestedReviewers, Self::Error> {
        let state = self.state();
        state.pull(&pr)?;
        Ok(state
            .requested_reviewers
            .get(&pr)
            .cloned()
            .unwrap_or_default())
    }

    async fn read_check_runs(&self, sha: RevId) -> Result<Vec<CheckRun>, Self::Error> {
        Ok(self
            .state()
            .check_runs
            .get(&sha)
            .cloned()
            .unwrap_or_default())
    }

    async fn read_check_suites(&self, sha: RevId) -> Result<Vec<CheckSuite>, Self::Error> {
        Ok(self
            .state()
            .check_suites
            .get(&sha)
            .cloned()
            .unwrap_or_default())
    }

    /// The seeded statuses; a commit with none seeded is `pending` with no entries.
    async fn read_statuses(&self, sha: RevId) -> Result<CombinedStatus, Self::Error> {
        Ok(self
            .state()
            .commit_statuses
            .get(&sha)
            .cloned()
            .unwrap_or_else(|| CombinedStatus {
                state: "pending".into(),
                total_count: 0,
                statuses: Vec::new(),
            }))
    }
}

impl CodeOwners for FakeTracker {
    /// The first of [`CODEOWNERS_PATHS`] seeded at `rev`, as the GitHub tracker reads them; a
    /// `rev` that is not a full commit id, by [`is_commit_id`] as the GitHub tracker checks it,
    /// is [`FakeTrackerError::InvalidRev`].
    async fn read_codeowners(&self, rev: RevId) -> Result<Option<CodeOwnersFile>, Self::Error> {
        if !is_commit_id(&rev.0) {
            return Err(FakeTrackerError::InvalidRev { rev });
        }
        let state = self.state();
        Ok(CODEOWNERS_PATHS.iter().find_map(|path| {
            state
                .files
                .get(&(rev.clone(), (*path).to_owned()))
                .map(|text| CodeOwnersFile {
                    path: RepoPath(path.as_bytes().to_vec()),
                    text: text.clone(),
                })
        }))
    }

    async fn read_changed_files(&self, pr: PrNumber) -> Result<Vec<ChangedFile>, Self::Error> {
        let state = self.state();
        state.pr_list(&pr, &state.changed_files)
    }
}

impl Merges for FakeTracker {
    type MergeToken = Token;

    /// Records the call, then answers the pull request's next script if one is queued, applying
    /// its effect on the pull request's state (see [`FakeMerge`]), and otherwise its state, in
    /// this order, as GitHub refuses (405) a pull request that cannot be merged whatever the
    /// `sha`:
    ///
    /// 1. a pull request that is merged, not `open`, or seeded with `mergeable: Some(false)` is
    ///    [`MergeOutcome::NotMergeable`];
    /// 2. a `sha` that is not the current head is [`MergeOutcome::HeadMoved`];
    /// 3. any other is merged and closed with the merge commit `format!("{:040x}", pr.0)`.
    ///
    /// A pull request that was not seeded is [`FakeTrackerError::NoSuchPull`], scripted or not.
    async fn merge(
        &self,
        token: &Token,
        pr: PrNumber,
        sha: RevId,
        title: String,
    ) -> Result<MergeOutcome, Self::Error> {
        let mut state = self.state();
        state.writes.push(FakeWrite::Merge {
            pr: pr.clone(),
            sha: sha.clone(),
            title,
            token: token.clone(),
        });
        state.merge(&pr, &sha)
    }

    fn may_have_landed(error: &FakeTrackerError) -> bool {
        matches!(error, FakeTrackerError::MergeLost { landed: true })
    }
}

/// Marks `pull` merged and closed with the merge commit `commit`.
fn mark_merged(pull: &mut PullDetail, commit: &RevId) {
    pull.merged = true;
    pull.state = "closed".into();
    pull.merge_commit = Some(commit.clone());
}
