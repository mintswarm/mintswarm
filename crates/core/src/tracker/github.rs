//! The GitHub tracker: the capability traits the plane uses ([`Issues`], [`Links`], [`Fields`],
//! [`Statuses`], [`Comments`], [`Reactions`], [`CommentEdits`] and [`Collaborators`]) over the
//! `mintswarm-github` clients, design §9.
//!
//! Every call asks its [`InstallationTokens`] for one installation token, writing issues and
//! reading the repository's metadata, and opens a fresh pass of the client it uses, so each call
//! reads GitHub again.
//!
//! A field-mode role is configured by its name only (design §7, "Field ids"). The tracker keeps
//! the ids it learned for the process, keyed by the field's name, and resolves an id it does not
//! hold in this order: from the issue's own `issue-field-values`, when they show the field, and
//! then from the organisation's field list. When that list is refused or does not name the field,
//! the call is deferred: it logs the operator's step and returns `field_missing`, and nothing is
//! written. An error for which [`FieldError::should_resolve_again`] holds makes the tracker forget
//! the id it used, so the next call resolves it again.
//!
//! Filing sub-issues, typing issues, linking dependencies and listing ready issues are not
//! supported yet: they return [`GitHubTrackerError::Unsupported`] and send nothing.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Mutex, MutexGuard};

use mintswarm_domain::field::{FieldError, FieldFinding, FieldRead, FieldRef, FieldSource};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{
    Artifact, Collaborators, Comment, CommentEdits, Comments, Cursor, Dependency, Direction,
    Fields, Issue, IssueRead, IssueType, Issues, Link, Links, Login, Reactions, RepoRole, Statuses,
    SubItem, TrackerBase,
};
use mintswarm_github::{
    CommentClient, CommentError, FieldClient, FieldPass, GitHubFieldCatalogue, GitHubReads,
    IssueWrites, NOTE_MARKER, Permissions, ReadError, STEER_MARKER, StatusClient, StatusError,
    Token, TokenClient, TokenError, WriteError,
};

/// The operator's step for a field whose id cannot be resolved, as the deferral log names it.
const FIELD_IDS_STEP: &str = "set the field on one issue by hand so its values show the field's \
     id; see .mintswarm/dogfood/github/README.md, \"Field ids\"";

/// Where the tracker gets its installation tokens.
pub trait InstallationTokens: Send + Sync {
    /// A token for `owner/repo` with `permissions` only.
    fn token(
        &self,
        owner: &str,
        repo: &str,
        permissions: &Permissions,
    ) -> impl Future<Output = Result<Token, TokenError>> + Send;
}

impl InstallationTokens for TokenClient {
    fn token(
        &self,
        owner: &str,
        repo: &str,
        permissions: &Permissions,
    ) -> impl Future<Output = Result<Token, TokenError>> + Send {
        TokenClient::token(self, owner, repo, permissions)
    }
}

/// The `mintswarm-github` clients the tracker calls, each bound to the tracker's repository.
#[derive(Debug)]
pub struct GitHubClients {
    /// Issues, their comments, links and dependencies.
    pub reads: GitHubReads,
    /// Role fields and labels.
    pub fields: FieldClient,
    /// Issue statuses.
    pub statuses: StatusClient,
    /// Notes.
    pub comments: CommentClient,
    /// Reactions, comment edits and artifact updates. It must share the
    /// [`ContentLimiter`](mintswarm_github::ContentLimiter) of `comments`, so both count against
    /// one content budget.
    pub writes: IssueWrites,
}

/// Why a tracker call failed. Each client's error is kept as it is, so its rate-limit and
/// exhaustion distinctions survive.
#[derive(Debug, thiserror::Error)]
pub enum GitHubTrackerError {
    /// No installation token could be had.
    #[error(transparent)]
    Token(#[from] TokenError),
    /// An issue, link or dependency read failed.
    #[error(transparent)]
    Read(#[from] ReadError),
    /// A status read or write failed.
    #[error(transparent)]
    Status(#[from] StatusError),
    /// A note could not be posted.
    #[error(transparent)]
    Comment(#[from] CommentError),
    /// A reaction, comment edit or artifact update failed; [`WriteError::Refused`] when the
    /// installation may not change the item.
    #[error(transparent)]
    Write(#[from] WriteError),
    /// The tracker does not support the operation yet; nothing was sent.
    #[error("the GitHub tracker does not support {operation}")]
    Unsupported {
        /// The trait method called.
        operation: &'static str,
    },
}

/// The plane's tracker over one GitHub repository.
#[derive(Debug)]
pub struct GitHubTracker<T: InstallationTokens> {
    api_base: String,
    owner: String,
    repo: String,
    tokens: T,
    clients: GitHubClients,
    /// The field ids learned so far, by field name, kept for the process.
    field_ids: Mutex<HashMap<String, u64>>,
}

impl<T: InstallationTokens> GitHubTracker<T> {
    /// A tracker for `owner/repo` on the API at `api_base` (normally
    /// [`GITHUB_API`](mintswarm_github::GITHUB_API)), with tokens from `tokens` and the
    /// `clients`, which must be bound to the same repository and API.
    pub fn new(api_base: &str, owner: &str, repo: &str, tokens: T, clients: GitHubClients) -> Self {
        Self {
            api_base: api_base.to_owned(),
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            tokens,
            clients,
            field_ids: Mutex::new(HashMap::new()),
        }
    }

    /// The permissions of the one token every call asks for: writing issues, and reading the
    /// repository's metadata.
    pub fn permissions() -> Permissions {
        Permissions::write(["issues"]).and(Permissions::read(["metadata"]))
    }

    async fn token(&self) -> Result<Token, TokenError> {
        self.tokens
            .token(&self.owner, &self.repo, &Self::permissions())
            .await
    }

    /// The call's token for a field read or write, its error as a field error.
    async fn field_token(&self) -> Result<Token, FieldError> {
        self.token().await.map_err(|error| match error {
            TokenError::Exhausted(exhausted) => FieldError::Exhausted(exhausted),
            other => FieldError::Transport(other.to_string()),
        })
    }

    fn ids(&self) -> MutexGuard<'_, HashMap<String, u64>> {
        // The map holds plain ids, so a panic elsewhere leaves nothing half-written.
        self.field_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stores `id` for `name` unless an id is already stored for it.
    fn learn(&self, name: &str, id: u64) {
        self.ids().entry(name.to_owned()).or_insert(id);
    }

    /// Forgets `name`'s id when `error` calls for resolving it again and the stored id is still
    /// `id`, so an id another call learned meanwhile stays.
    fn forget_on(&self, name: &str, id: u64, error: &FieldError) {
        if !error.should_resolve_again() {
            return;
        }
        let mut ids = self.ids();
        if ids.get(name) == Some(&id) {
            ids.remove(name);
        }
    }

    /// The id of the field named `name`: the stored one, else the one `issue`'s values show,
    /// else the organisation's. A refused or fruitless organisation read defers the call.
    async fn field_id(
        &self,
        pass: &FieldPass<'_>,
        token: &Token,
        issue: &IssueNumber,
        name: &str,
    ) -> Result<u64, FieldError> {
        if let Some(id) = self.ids().get(name).copied() {
            return Ok(id);
        }
        if let Some(id) = pass.issue_field_id(issue.clone(), name).await? {
            self.learn(name, id);
            return Ok(id);
        }
        let catalogue =
            GitHubFieldCatalogue::new(&self.api_base, &self.owner, &self.repo, token.clone())
                .map_err(|error| FieldError::Transport(error.to_string()))?;
        match catalogue.field_id(name).await {
            Ok(id) => {
                self.learn(name, id);
                Ok(id)
            }
            Err(error @ FieldError::Field(FieldFinding::FieldMissing)) => {
                tracing::warn!(
                    field = name,
                    issue = issue.0,
                    "the field's id is not known and the organisation's field list does not \
                     give it, so the issue is deferred and nothing is written: {FIELD_IDS_STEP}"
                );
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
}

impl<T: InstallationTokens> TrackerBase for GitHubTracker<T> {
    type Error = GitHubTrackerError;
}

impl<T: InstallationTokens> Issues for GitHubTracker<T> {
    async fn fetch_ready(&self) -> Result<Vec<Issue>, Self::Error> {
        Err(GitHubTrackerError::Unsupported {
            operation: "fetch_ready",
        })
    }

    async fn read_issue(
        &self,
        issue: IssueNumber,
        since: Option<Cursor>,
    ) -> Result<IssueRead, Self::Error> {
        let token = self.token().await?;
        Ok(self.clients.reads.read_issue(&token, issue, since).await?)
    }

    async fn create_sub_item(
        &self,
        _parent: IssueNumber,
        _item: SubItem,
    ) -> Result<Issue, Self::Error> {
        Err(GitHubTrackerError::Unsupported {
            operation: "create_sub_item",
        })
    }

    async fn set_issue_type(
        &self,
        _issue: IssueNumber,
        _kind: IssueType,
    ) -> Result<(), Self::Error> {
        Err(GitHubTrackerError::Unsupported {
            operation: "set_issue_type",
        })
    }
}

impl<T: InstallationTokens> Links for GitHubTracker<T> {
    async fn read_links(&self, issue: IssueNumber) -> Result<Vec<Link>, Self::Error> {
        let token = self.token().await?;
        Ok(self.clients.reads.read_links(&token, issue).await?)
    }

    async fn link_dependency(
        &self,
        _issue: IssueNumber,
        _blocker: IssueNumber,
    ) -> Result<(), Self::Error> {
        Err(GitHubTrackerError::Unsupported {
            operation: "link_dependency",
        })
    }

    async fn read_dependencies(
        &self,
        issue: IssueNumber,
        direction: Direction,
    ) -> Result<Vec<Dependency>, Self::Error> {
        let token = self.token().await?;
        Ok(self
            .clients
            .reads
            .read_dependencies(&token, issue, direction)
            .await?)
    }
}

impl<T: InstallationTokens> Statuses for GitHubTracker<T> {
    async fn read_status(&self, issue: IssueNumber) -> Result<Option<String>, Self::Error> {
        let token = self.token().await?;
        Ok(self
            .clients
            .statuses
            .pass(&token)
            .read_status(issue)
            .await?)
    }

    async fn set_status(&self, issue: IssueNumber, status: String) -> Result<(), Self::Error> {
        let token = self.token().await?;
        Ok(self
            .clients
            .statuses
            .pass(&token)
            .set_status(issue, &status)
            .await?)
    }
}

impl<T: InstallationTokens> Comments for GitHubTracker<T> {
    /// Posts `body` as a note, keyed by its first key line ([`note_key`]); a body without one is
    /// [`CommentError::Unmarked`] before any request.
    async fn post_comment(&self, issue: IssueNumber, body: String) -> Result<Comment, Self::Error> {
        let key = note_key(&body).ok_or(CommentError::Unmarked)?;
        let token = self.token().await?;
        Ok(self
            .clients
            .comments
            .post_note(&token, issue, key, &body)
            .await?)
    }
}

impl<T: InstallationTokens> Reactions for GitHubTracker<T> {
    async fn react_plus_one(&self, comment_id: u64) -> Result<(), Self::Error> {
        let token = self.token().await?;
        Ok(self
            .clients
            .writes
            .react_plus_one(&token, comment_id)
            .await?)
    }
}

impl<T: InstallationTokens> CommentEdits for GitHubTracker<T> {
    async fn edit_comment(&self, comment_id: u64, body: String) -> Result<Comment, Self::Error> {
        let token = self.token().await?;
        Ok(self
            .clients
            .writes
            .edit_comment(&token, comment_id, &body)
            .await?)
    }

    async fn update_artifact(
        &self,
        issue: IssueNumber,
        number: u64,
        body: String,
    ) -> Result<Artifact, Self::Error> {
        let token = self.token().await?;
        Ok(self
            .clients
            .writes
            .update_artifact(&token, issue, number, &body)
            .await?)
    }
}

impl<T: InstallationTokens> Collaborators for GitHubTracker<T> {
    async fn read_permission(&self, login: Login) -> Result<RepoRole, Self::Error> {
        let token = self.token().await?;
        Ok(self.clients.reads.read_permission(&token, &login).await?)
    }

    async fn read_parent(&self, issue: IssueNumber) -> Result<Option<IssueNumber>, Self::Error> {
        let token = self.token().await?;
        Ok(self.clients.reads.read_parent(&token, issue).await?)
    }
}

impl<T: InstallationTokens> Fields for GitHubTracker<T> {
    async fn read_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
    ) -> Result<FieldRead, FieldError> {
        let token = self.field_token().await?;
        let pass = self.clients.fields.pass(&token);
        let FieldSource::Field { name } = &field.source else {
            return pass.read_field(issue, field, None).await;
        };
        let id = self.field_id(&pass, &token, &issue, name).await?;
        pass.read_field(issue, field, Some(id))
            .await
            .inspect_err(|error| self.forget_on(name, id, error))
    }

    async fn set_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
        option: Option<&str>,
    ) -> Result<(), FieldError> {
        let token = self.field_token().await?;
        let pass = self.clients.fields.pass(&token);
        let FieldSource::Field { name } = &field.source else {
            return pass.set_field(issue, field, None, option).await;
        };
        let id = self.field_id(&pass, &token, &issue, name).await?;
        pass.set_field(issue, field, Some(id), option)
            .await
            .inspect_err(|error| self.forget_on(name, id, error))
    }
}

/// The key of a note: its first line, split at LF and trimmed, that is a mintswarm marker
/// (`<!-- mintswarm: … -->`) other than [`NOTE_MARKER`] and [`STEER_MARKER`].
pub(crate) fn note_key(body: &str) -> Option<&str> {
    body.split('\n').map(str::trim).find(|line| {
        line.starts_with("<!-- mintswarm:")
            && line.ends_with("-->")
            && *line != NOTE_MARKER
            && *line != STEER_MARKER
    })
}

#[cfg(test)]
mod tests {
    use super::{NOTE_MARKER, STEER_MARKER, note_key};

    #[test]
    fn the_key_is_the_first_marker_that_is_not_a_kind_marker() {
        let body = format!("{NOTE_MARKER}\n<!-- mintswarm: plan 12 -->\n<!-- mintswarm: x -->\n");
        assert_eq!(note_key(&body), Some("<!-- mintswarm: plan 12 -->"));
    }

    #[test]
    fn a_body_with_the_note_marker_only_has_no_key() {
        assert_eq!(note_key(&format!("{NOTE_MARKER}\nSome text.")), None);
    }

    #[test]
    fn a_body_with_the_steer_marker_only_has_no_key() {
        assert_eq!(note_key(&format!("{STEER_MARKER}\nSome text.")), None);
    }

    #[test]
    fn a_key_line_is_trimmed() {
        let body = format!("{NOTE_MARKER}\r\n   <!-- mintswarm: state -->  \r\nText.");
        assert_eq!(note_key(&body), Some("<!-- mintswarm: state -->"));
    }
}
