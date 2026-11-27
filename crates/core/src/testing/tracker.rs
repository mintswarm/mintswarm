//! An in-memory tracker for tests: issues with their comments, links, dependencies, role field
//! values and statuses, seeded by the test, with every accepted write recorded in order.
//!
//! [`FakeTracker`] implements the capability traits the GitHub tracker does ([`Issues`],
//! [`Links`], [`Fields`], [`Statuses`], [`Comments`], [`Reactions`], [`CommentEdits`] and
//! [`Collaborators`]), and refuses what it does: an option the role's map does not list, a
//! status the repository does not have, a comment without a note key, and the operations the
//! GitHub tracker does not support yet. A test may also make it refuse a comment's reactions and
//! edits, or an artifact's updates, as GitHub refuses a change the installation may not make. It
//! does not model labels, so a labels-mode role never reads as a conflict.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use mintswarm_domain::field::{
    FieldError, FieldFinding, FieldRead, FieldRef, FieldSource, FieldValue,
};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{
    Artifact, Collaborators, Comment, CommentEdits, Comments, Cursor, Dependency, Direction,
    Fields, Issue, IssueRead, IssueType, Issues, Link, LinkKind, Links, Login, Reactions, RepoRole,
    Statuses, SubItem, TrackerBase,
};
use mintswarm_github::NOTE_MARKER;

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
    /// The comments carrying the bot's `+1`.
    reacted: HashSet<u64>,
    artifacts: HashMap<(IssueNumber, u64), Artifact>,
    /// The logins' roles; an unset login is [`RepoRole::NoAccess`].
    roles: HashMap<Login, RepoRole>,
    refused_comments: HashSet<u64>,
    refused_artifacts: HashSet<(IssueNumber, u64)>,
    now: String,
    next_comment: u64,
    writes: Vec<FakeWrite>,
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
                reacted: HashSet::new(),
                artifacts: HashMap::new(),
                roles: HashMap::new(),
                refused_comments: HashSet::new(),
                refused_artifacts: HashSet::new(),
                now: START.to_owned(),
                next_comment: 1,
                writes: Vec::new(),
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

    /// The RFC 3339 time stamped on comments posted or edited from now on.
    pub fn set_now(&self, now: &str) {
        self.state().now = now.to_owned();
    }

    /// Every accepted write, in order.
    pub fn writes(&self) -> Vec<FakeWrite> {
        self.state().writes.clone()
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
        Ok(self
            .state()
            .roles
            .get(&login)
            .copied()
            .unwrap_or(RepoRole::NoAccess))
    }

    /// The issue's seeded [`LinkKind::Parent`] link, so it agrees with [`Links::read_links`].
    async fn read_parent(&self, issue: IssueNumber) -> Result<Option<IssueNumber>, Self::Error> {
        let state = self.state();
        state.issue(&issue)?;
        Ok(state.links.get(&issue).and_then(|links| {
            links
                .iter()
                .find(|link| link.kind == LinkKind::Parent)
                .map(|link| IssueNumber(link.number))
        }))
    }
}

impl Fields for FakeTracker {
    async fn read_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
    ) -> Result<FieldRead, FieldError> {
        let state = self.state();
        state.field_check(&issue)?;
        let Some(name) = state.fields.get(&(issue, field.source.clone())) else {
            return Ok(FieldRead::Unset);
        };
        if !field.options.contains(name) {
            return Err(FieldError::Field(FieldFinding::OptionUnmapped));
        }
        Ok(FieldRead::Value(FieldValue {
            field_id: None,
            option_id: None,
            name: name.clone(),
        }))
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
