//! Reads of one repository's issues, comments, links, dependencies and artifacts as the runner
//! App, design §9 "Reads and writes": an issue with its comments, the issues that block it or that
//! it blocks, and a listing of issues by state, update time, labels and creator, which drops pull
//! requests and gives GitHub's clock at its first reply. The reads are GETs, plus read-only
//! GraphQL queries for what REST does not show, such as whether a review thread is resolved; a
//! GraphQL reply with `errors` is [`ReadError::GraphQl`].
//!
//! Every GET goes through one ETag cache, so a repeated read is revalidated with
//! `If-None-Match` and a 304 is served from the cache. Lists are read with `per_page=100`, page
//! by page, following the `Link` header's `rel="next"`, and a list with a page after its 30th
//! is refused. Every GET goes through the retry layer: a transient failure is retried within the
//! client's [`RetryPolicy`], and a read whose retries run out fails with [`ReadError::Exhausted`].
//! After a rate-limited reply that names a wait, the client sends nothing until that long has
//! passed.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;

use mintswarm_domain::field::FieldError;
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::retry::{Exhausted, RetryPolicy};
use mintswarm_domain::tracker::{
    Artifact, ArtifactSummary, Comment, CommentMark, Cursor, Dependency, Direction, Issue,
    IssueRead, IssueType, Link, LinkKind, Login,
};

use crate::etag::{ETAG_CACHE_LIMIT, EtagCache};
use crate::rest::{ClientError, Reply, Rest, check_names, path_segment};
use crate::secret::Token;
use crate::token::{Permissions, http_date_to_rfc3339};

/// Items asked for per page.
pub const PER_PAGE: usize = 100;
/// At most this many pages are read of one list.
pub const MAX_PAGES: u32 = 30;

/// The marker line that makes a bot comment a steer.
pub const STEER_MARKER: &str = "<!-- mintswarm: steer -->";
/// The marker line that makes a bot comment a note.
pub const NOTE_MARKER: &str = "<!-- mintswarm: note -->";

/// Why a read failed. No variant carries a token or a response body.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReadError {
    /// The request could not be sent or its answer not read.
    #[error("transport error: {0}")]
    Transport(String),
    /// GitHub has no such resource, or does not show it to the token.
    #[error("{endpoint}: not found")]
    NotFound {
        /// The method and path.
        endpoint: String,
    },
    /// GitHub answered with another status that is not a success.
    #[error("{endpoint}: status {status}")]
    Status {
        /// The method and path.
        endpoint: String,
        /// The HTTP status.
        status: u16,
    },
    /// The read kept failing transiently, on server errors, broken connections, timeouts or
    /// rate limits, until its retries ran out; a rate limit's named wait also pauses later
    /// reads.
    #[error(transparent)]
    Exhausted(#[from] Exhausted),
    /// The list needs more than [`MAX_PAGES`] pages.
    #[error("{endpoint}: more than {MAX_PAGES} pages")]
    TooManyPages {
        /// The method and path, without the page.
        endpoint: String,
    },
    /// The answer is not JSON of the expected shape.
    #[error("{endpoint}: not the expected JSON")]
    Unexpected {
        /// The method and path.
        endpoint: String,
    },
    /// GraphQL refused a query with errors other than a rate limit.
    #[error("{endpoint}: GraphQL errors of types {types:?}")]
    GraphQl {
        /// The method and path.
        endpoint: String,
        /// The `type` of each error, in order; GitHub's messages are not kept.
        types: Vec<String>,
    },
    /// A listing asked for a label holding a comma, which GitHub would split into two labels;
    /// nothing is sent.
    #[error("label {label:?} holds a comma and cannot be listed by")]
    CommaInLabel {
        /// The label asked for.
        label: String,
    },
}

impl From<FieldError> for ReadError {
    fn from(error: FieldError) -> Self {
        match error {
            FieldError::Transport(message) => Self::Transport(message),
            FieldError::Exhausted(exhausted) => Self::Exhausted(exhausted),
            other => Self::Transport(other.to_string()),
        }
    }
}

/// Where an issue's kind is read, from `tracker.kinds`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindSource {
    /// GitHub's issue type, by its name.
    IssueTypes,
    /// A label.
    Labels,
}

/// How an issue's GitHub type or labels map to its kind, built from `tracker.kinds`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kinds {
    source: KindSource,
    names: Vec<(IssueType, String)>,
}

impl Kinds {
    /// Kinds read from GitHub's issue type, each kind with its type's name.
    pub fn issue_types(names: impl IntoIterator<Item = (IssueType, String)>) -> Self {
        Self {
            source: KindSource::IssueTypes,
            names: names.into_iter().collect(),
        }
    }

    /// Kinds read from labels, each kind with its label.
    pub fn labels(names: impl IntoIterator<Item = (IssueType, String)>) -> Self {
        Self {
            source: KindSource::Labels,
            names: names.into_iter().collect(),
        }
    }

    /// Where the kind is read.
    pub fn source(&self) -> KindSource {
        self.source
    }

    fn kind_of(&self, name: &str) -> Option<IssueType> {
        self.names
            .iter()
            .find(|(_, known)| known.eq_ignore_ascii_case(name))
            .map(|(kind, _)| *kind)
    }

    /// The kind of an issue with GitHub type `type_name` and `labels`: the type's in issue-types
    /// mode, the first matching label's in labels mode, and `None` without a match.
    fn resolve(&self, type_name: Option<&str>, labels: &[String]) -> Option<IssueType> {
        match self.source {
            KindSource::IssueTypes => type_name.and_then(|name| self.kind_of(name)),
            KindSource::Labels => labels.iter().find_map(|label| self.kind_of(label)),
        }
    }
}

/// Whether `comment` is a steer, a note or neither.
///
/// A comment by a `trusted` login is a steer. A comment by `bot` is a steer or a note when one
/// of its lines, split at LF and trimmed, is exactly [`STEER_MARKER`] or [`NOTE_MARKER`]; a
/// marker inside a line does not count, and a marker on anyone else's comment is ignored.
/// Logins are compared ASCII case-insensitively, as GitHub treats them.
pub fn comment_mark(comment: &Comment, trusted: &[Login], bot: &Login) -> Option<CommentMark> {
    let author = &comment.author.0;
    if trusted
        .iter()
        .any(|login| login.0.eq_ignore_ascii_case(author))
    {
        return Some(CommentMark::Steer);
    }
    if !bot.0.eq_ignore_ascii_case(author) {
        return None;
    }
    let has = |marker: &str| comment.body.split('\n').any(|line| line.trim() == marker);
    if has(STEER_MARKER) {
        Some(CommentMark::Steer)
    } else if has(NOTE_MARKER) {
        Some(CommentMark::Note)
    } else {
        None
    }
}

/// Which issues a listing asks for, by state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueState {
    /// Open issues only.
    Open,
    /// Open and closed issues.
    All,
}

impl IssueState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::All => "all",
        }
    }
}

/// What [`GitHubReads::list_issues`] asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueQuery {
    pub state: IssueState,
    /// Only issues updated at or after this time, as `YYYY-MM-DDTHH:MM:SSZ`.
    pub since: Option<String>,
    /// Only issues carrying every one of these labels. GitHub splits the list at commas, so a
    /// label holding one is refused with [`ReadError::CommaInLabel`] before anything is sent.
    pub labels: Vec<String>,
    /// Only issues opened by this login.
    pub creator: Option<Login>,
}

/// The issues a listing found, with GitHub's clock at its first reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueList {
    pub issues: Vec<Issue>,
    /// The first reply's `Date`, as `YYYY-MM-DDTHH:MM:SSZ`: a checkpoint for the next listing's
    /// `since` on GitHub's clock, not this host's.
    pub date: String,
}

/// Reads of one repository with a token its caller passes in: GETs, and read-only GraphQL
/// queries.
#[derive(Debug)]
pub struct GitHubReads {
    pub(crate) rest: Rest,
    owner: String,
    repo: String,
    kinds: Kinds,
    trusted: Vec<Login>,
    bot: Login,
    /// The last ETag and page of each path read, kept across reads.
    etags: Mutex<EtagCache<String, Page>>,
}

/// A list read page by page: every item, and the first page's whole body and raw `Date` header.
#[derive(Debug)]
pub(crate) struct Paged {
    pub items: Vec<Value>,
    /// The first page's JSON, for the fields beside the list, such as a `total_count`.
    pub first: Arc<Value>,
    pub date: Option<String>,
}

/// One read's JSON and whether the list it is a page of goes on.
#[derive(Clone, Debug)]
struct Page {
    body: Arc<Value>,
    /// The `Link` header's answer: whether a `rel="next"` page exists; `None` without a `Link`.
    next: Option<bool>,
}

impl GitHubReads {
    /// Reads of `owner/repo` on the API at `api_base` (normally
    /// [`GITHUB_API`](crate::GITHUB_API)): `kinds` maps issue kinds, and `trusted` and `bot`
    /// are the logins [`comment_mark`] is given. It retries with the default [`RetryPolicy`].
    pub fn new(
        api_base: &str,
        owner: &str,
        repo: &str,
        kinds: Kinds,
        trusted: Vec<Login>,
        bot: Login,
    ) -> Result<Self, ClientError> {
        check_names(owner, repo)?;
        Ok(Self {
            rest: Rest::pausing(api_base, RetryPolicy::default())?,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            kinds,
            trusted,
            bot,
            etags: Mutex::new(EtagCache::new(ETAG_CACHE_LIMIT)),
        })
    }

    /// The reads retrying with `policy` in place of the default.
    #[must_use]
    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.rest = self.rest.with_policy(policy);
        self
    }

    /// The permissions a token for these reads asks for, from its
    /// [`TokenClient`](crate::TokenClient).
    pub fn permissions() -> Permissions {
        Permissions::read(["issues", "pull_requests", "checks", "statuses", "metadata"])
    }

    /// What is left of the pause a rate limit's named wait started; zero when there is none.
    pub fn pause_remaining(&self) -> Duration {
        self.rest.pause_remaining()
    }

    /// Whether `comment` is a steer, a note or neither, with this client's logins.
    pub fn mark(&self, comment: &Comment) -> Option<CommentMark> {
        comment_mark(comment, &self.trusted, &self.bot)
    }

    /// `issue` and its comments updated at or after `since`, all of them without it.
    ///
    /// `since` is inclusive, so a comment at the cursor comes back again, as does any edited
    /// since: callers deduplicate by [`Comment::id`]. The new cursor is the latest `updated_at`
    /// seen; with no comments it is `since`, or the issue's `created_at` when there is none.
    pub async fn read_issue(
        &self,
        token: &Token,
        issue: IssueNumber,
        since: Option<Cursor>,
    ) -> Result<IssueRead, ReadError> {
        let path = self.issue_path(issue.0);
        let body = self.get(token, &path).await?;
        let endpoint = format!("GET {path}");
        let (read, created_at) = self.map_issue(&body, &endpoint)?;
        let mut comments_path = format!("{path}/comments");
        if let Some(Cursor(since)) = &since {
            comments_path.push_str(&format!("?since={}", path_segment(since)));
        }
        let comments = self
            .get_paged(token, &comments_path, None)
            .await?
            .iter()
            .map(|comment| map_comment(comment, &format!("GET {comments_path}")))
            .collect::<Result<Vec<_>, _>>()?;
        let cursor = comments
            .iter()
            .map(|comment| comment.updated_at.as_str())
            .max()
            .map(|latest| Cursor(latest.to_owned()))
            .or(since)
            .unwrap_or(Cursor(created_at));
        Ok(IssueRead {
            issue: read,
            comments,
            cursor,
        })
    }

    /// The issues that block `issue`, or that it blocks, from `dependencies/blocked_by` or
    /// `dependencies/blocking`. A dependency may be in another repository.
    pub async fn read_dependencies(
        &self,
        token: &Token,
        issue: IssueNumber,
        direction: Direction,
    ) -> Result<Vec<Dependency>, ReadError> {
        let leaf = match direction {
            Direction::BlockedBy => "blocked_by",
            Direction::Blocking => "blocking",
        };
        let path = format!("{}/dependencies/{leaf}", self.issue_path(issue.0));
        let endpoint = format!("GET {path}");
        self.get_paged(token, &path, None)
            .await?
            .iter()
            .map(|dependency| map_dependency(dependency, &endpoint))
            .collect()
    }

    /// The repository's issues that `query` asks for, without pull requests, and GitHub's clock
    /// at the first reply.
    ///
    /// The date is the first reply's `Date` even when that reply is a 304 served from the ETag
    /// cache; a reply without a `Date` in IMF-fixdate form is [`ReadError::Unexpected`]. A label
    /// holding a comma is [`ReadError::CommaInLabel`], and nothing is sent.
    pub async fn list_issues(
        &self,
        token: &Token,
        query: &IssueQuery,
    ) -> Result<IssueList, ReadError> {
        if let Some(label) = query.labels.iter().find(|label| label.contains(',')) {
            return Err(ReadError::CommaInLabel {
                label: label.clone(),
            });
        }
        let mut path = format!("{}/issues?state={}", self.repo_path(), query.state.as_str());
        if let Some(since) = &query.since {
            path.push_str(&format!("&since={}", path_segment(since)));
        }
        if !query.labels.is_empty() {
            let labels = query
                .labels
                .iter()
                .map(|label| path_segment(label))
                .collect::<Vec<_>>()
                .join(",");
            path.push_str(&format!("&labels={labels}"));
        }
        if let Some(Login(creator)) = &query.creator {
            path.push_str(&format!("&creator={}", path_segment(creator)));
        }
        let endpoint = format!("GET {path}");
        let (items, date) = self.get_paged_dated(token, &path, None).await?;
        let date = date
            .as_deref()
            .and_then(http_date_to_rfc3339)
            .ok_or_else(|| unexpected(&endpoint))?;
        let issues = items
            .iter()
            .filter(|item| item.get("pull_request").is_none_or(Value::is_null))
            .map(|item| self.map_issue(item, &endpoint).map(|(issue, _)| issue))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(IssueList { issues, date })
    }

    /// `issue`'s parent, if it has one, and its sub-issues.
    pub async fn read_links(
        &self,
        token: &Token,
        issue: IssueNumber,
    ) -> Result<Vec<Link>, ReadError> {
        let path = self.issue_path(issue.0);
        let mut links = Vec::new();
        let parent_path = format!("{path}/parent");
        match self.get(token, &parent_path).await {
            Ok(parent) => links.push(Link {
                kind: LinkKind::Parent,
                number: u64_at(&parent, "number", &format!("GET {parent_path}"))?,
            }),
            Err(ReadError::NotFound { .. }) => {}
            Err(error) => return Err(error),
        }
        let subs_path = format!("{path}/sub_issues");
        for sub in self.get_paged(token, &subs_path, None).await? {
            links.push(Link {
                kind: LinkKind::SubItem,
                number: u64_at(&sub, "number", &format!("GET {subs_path}"))?,
            });
        }
        Ok(links)
    }

    /// The artifacts published on `issue`, without their bodies.
    pub async fn list_artifacts(
        &self,
        token: &Token,
        issue: IssueNumber,
    ) -> Result<Vec<ArtifactSummary>, ReadError> {
        let path = format!("{}/artifacts", self.issue_path(issue.0));
        let endpoint = format!("GET {path}");
        self.get_paged(token, &path, None)
            .await?
            .iter()
            .map(|artifact| map_artifact_summary(artifact, &endpoint))
            .collect()
    }

    /// The artifact numbered `number` on `issue` (its per-issue number, not its id).
    pub async fn read_artifact(
        &self,
        token: &Token,
        issue: IssueNumber,
        number: u64,
    ) -> Result<Artifact, ReadError> {
        let path = format!("{}/artifacts/{number}", self.issue_path(issue.0));
        let body = self.get(token, &path).await?;
        map_artifact(&body, &format!("GET {path}"))
    }

    pub(crate) fn owner(&self) -> &str {
        &self.owner
    }

    pub(crate) fn repo(&self) -> &str {
        &self.repo
    }

    pub(crate) fn repo_path(&self) -> String {
        format!("/repos/{}/{}", self.owner, self.repo)
    }

    fn issue_path(&self, issue: u64) -> String {
        format!("{}/issues/{issue}", self.repo_path())
    }

    fn map_issue(&self, body: &Value, endpoint: &str) -> Result<(Issue, String), ReadError> {
        let bad = || unexpected(endpoint);
        let labels = body
            .get("labels")
            .and_then(Value::as_array)
            .ok_or_else(bad)?
            .iter()
            .map(|label| match label {
                Value::String(name) => Some(name.clone()),
                other => other.get("name").and_then(Value::as_str).map(str::to_owned),
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(bad)?;
        let type_name = body
            .get("type")
            .and_then(|kind| kind.get("name"))
            .and_then(Value::as_str);
        let issue_type = self.kinds.resolve(type_name, &labels);
        let issue = Issue {
            number: IssueNumber(u64_at(body, "number", endpoint)?),
            title: str_at(body, "title", endpoint)?,
            body: text_at(body, "body", endpoint)?,
            state: str_at(body, "state", endpoint)?,
            labels,
            issue_type,
            author: login_at(body, "user", endpoint)?,
            url: str_at(body, "html_url", endpoint)?,
            sub_issues: summary_total_at(body, "sub_issues_summary", "total", endpoint)?,
            blocked_by: summary_total_at(
                body,
                "issue_dependencies_summary",
                "total_blocked_by",
                endpoint,
            )?,
        };
        Ok((issue, str_at(body, "created_at", endpoint)?))
    }

    /// The JSON at `path`, revalidated against the ETag cache.
    pub(crate) async fn get(&self, token: &Token, path: &str) -> Result<Arc<Value>, ReadError> {
        Ok(self.get_page(token, path).await?.0.body)
    }

    /// The JSON at `path` with its next-page hint, revalidated against the ETag cache, and the
    /// reply's own `Date`, which is never cached: a 304 gives its own.
    async fn get_page(
        &self,
        token: &Token,
        path: &str,
    ) -> Result<(Page, Option<String>), ReadError> {
        let cached = self.cache().get(&path.to_owned());
        let reply = self
            .rest
            .get(path, token, cached.as_ref().map(|(etag, _)| etag.as_str()))
            .await?;
        if reply.status == StatusCode::NOT_MODIFIED {
            let (_, page) = cached.ok_or_else(|| {
                ReadError::Transport(format!("{}: 304 with nothing cached", reply.endpoint))
            })?;
            return Ok((page, reply.date));
        }
        if reply.status.is_success() {
            let page = Page {
                body: Arc::new(
                    serde_json::from_slice::<Value>(&reply.body)
                        .map_err(|_| unexpected(&reply.endpoint))?,
                ),
                next: reply.next_page,
            };
            let mut cache = self.cache();
            match reply.etag {
                Some(etag) => cache.insert(path.to_owned(), etag, page.clone()),
                None => cache.remove(&path.to_owned()),
            }
            return Ok((page, reply.date));
        }
        Err(if reply.status == StatusCode::NOT_FOUND {
            ReadError::NotFound {
                endpoint: reply.endpoint,
            }
        } else {
            ReadError::Status {
                endpoint: reply.endpoint,
                status: reply.status.as_u16(),
            }
        })
    }

    /// Every item of the list at `path` (which may carry a query), [`PER_PAGE`] a page: a bare
    /// array, or the array under `items` of an object. A page is the last when its `Link`
    /// header has no `rel="next"`, or, without a `Link`, when it is short. A list that goes on
    /// after page [`MAX_PAGES`] is refused without asking for the next page.
    pub(crate) async fn get_paged(
        &self,
        token: &Token,
        path: &str,
        items: Option<&str>,
    ) -> Result<Vec<Value>, ReadError> {
        Ok(self.get_paged_first(token, path, items).await?.items)
    }

    /// The list [`GitHubReads::get_paged`] reads, with the first reply's raw `Date` header.
    async fn get_paged_dated(
        &self,
        token: &Token,
        path: &str,
        items: Option<&str>,
    ) -> Result<(Vec<Value>, Option<String>), ReadError> {
        let paged = self.get_paged_first(token, path, items).await?;
        Ok((paged.items, paged.date))
    }

    /// The list [`GitHubReads::get_paged`] reads, with the first page's whole body and raw
    /// `Date` header.
    pub(crate) async fn get_paged_first(
        &self,
        token: &Token,
        path: &str,
        items: Option<&str>,
    ) -> Result<Paged, ReadError> {
        let separator = if path.contains('?') { '&' } else { '?' };
        let mut all = Vec::new();
        let mut first = None;
        for number in 1..=MAX_PAGES {
            let page_path = format!("{path}{separator}per_page={PER_PAGE}&page={number}");
            let (page, date) = self.get_page(token, &page_path).await?;
            let list = match items {
                Some(key) => page.body.get(key),
                None => Some(page.body.as_ref()),
            }
            .and_then(Value::as_array)
            .ok_or_else(|| unexpected(&format!("GET {page_path}")))?;
            all.extend(list.iter().cloned());
            let last = !page.next.unwrap_or(list.len() >= PER_PAGE);
            if first.is_none() {
                first = Some((Arc::clone(&page.body), date));
            }
            if last {
                // Set on page 1 above; the fallback is never taken.
                let (body, date) = first.unwrap_or((page.body, None));
                return Ok(Paged {
                    items: all,
                    first: body,
                    date,
                });
            }
        }
        Err(ReadError::TooManyPages {
            endpoint: format!("GET {path}"),
        })
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, EtagCache<String, Page>> {
        self.etags.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One entry of `GET issues/{n}/dependencies/blocked_by` or `/blocking`: an issue, with its
/// repository read from the `owner/name` after `/repos/` in its `repository_url`.
fn map_dependency(body: &Value, endpoint: &str) -> Result<Dependency, ReadError> {
    let url = str_at(body, "repository_url", endpoint)?;
    let (owner, name) = url
        .split_once("/repos/")
        .and_then(|(_, repository)| repository.split_once('/'))
        .filter(|(owner, name)| check_names(owner, name).is_ok())
        .ok_or_else(|| unexpected(endpoint))?;
    Ok(Dependency {
        repository: format!("{owner}/{name}"),
        number: IssueNumber(u64_at(body, "number", endpoint)?),
        open: str_at(body, "state", endpoint)? == "open",
    })
}

pub(crate) fn map_comment(body: &Value, endpoint: &str) -> Result<Comment, ReadError> {
    Ok(Comment {
        id: u64_at(body, "id", endpoint)?,
        author: login_at(body, "user", endpoint)?,
        body: text_at(body, "body", endpoint)?,
        created_at: str_at(body, "created_at", endpoint)?,
        updated_at: str_at(body, "updated_at", endpoint)?,
        url: str_at(body, "html_url", endpoint)?,
    })
}

/// One entry of `GET issues/{n}/artifacts`.
fn map_artifact_summary(body: &Value, endpoint: &str) -> Result<ArtifactSummary, ReadError> {
    Ok(ArtifactSummary {
        id: u64_at(body, "id", endpoint)?,
        number: u64_at(body, "number", endpoint)?,
        name: str_at(body, "name", endpoint)?,
        url: str_at(body, "html_url", endpoint)?,
    })
}

/// The answer of `GET issues/{n}/artifacts/{number}`: the artifact under `artifact`, beside its
/// versions.
fn map_artifact(body: &Value, endpoint: &str) -> Result<Artifact, ReadError> {
    let artifact = body.get("artifact").ok_or_else(|| unexpected(endpoint))?;
    Ok(Artifact {
        name: str_at(artifact, "name", endpoint)?,
        body: text_at(artifact, "body", endpoint)?,
    })
}

/// What a GraphQL reply says, before a caller maps it to its own error type.
#[derive(Debug)]
pub(crate) enum GraphQlReply {
    /// The reply's `data`, an object.
    Data(Value),
    /// The status is not a success.
    Refused,
    /// A non-empty `errors`, with only the errors' types.
    Errors(Vec<String>),
    /// A body that is not JSON, an `errors` that is not an array, or a `data` that is not an
    /// object.
    Unexpected,
}

/// Reads a GraphQL reply into a [`GraphQlReply`].
pub(crate) fn graphql_reply(reply: &Reply) -> GraphQlReply {
    if !reply.status.is_success() {
        return GraphQlReply::Refused;
    }
    let Ok(mut body) = serde_json::from_slice::<Value>(&reply.body) else {
        return GraphQlReply::Unexpected;
    };
    match body.get("errors") {
        None | Some(Value::Null) => {}
        Some(Value::Array(errors)) if errors.is_empty() => {}
        Some(Value::Array(errors)) => {
            return GraphQlReply::Errors(
                errors
                    .iter()
                    .map(|error| {
                        error
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned()
                    })
                    .collect(),
            );
        }
        Some(_) => return GraphQlReply::Unexpected,
    }
    match body.get_mut("data").map(Value::take) {
        Some(data @ Value::Object(_)) => GraphQlReply::Data(data),
        _ => GraphQlReply::Unexpected,
    }
}

pub(crate) fn unexpected(endpoint: &str) -> ReadError {
    ReadError::Unexpected {
        endpoint: endpoint.to_owned(),
    }
}

/// The string at `key`.
pub(crate) fn str_at(body: &Value, key: &str, endpoint: &str) -> Result<String, ReadError> {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| unexpected(endpoint))
}

/// The string at `key`, or empty for `null` or a missing key.
pub(crate) fn text_at(body: &Value, key: &str, endpoint: &str) -> Result<String, ReadError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(unexpected(endpoint)),
    }
}

/// The string at `key`, `None` for `null` or a missing key.
pub(crate) fn opt_str_at(
    body: &Value,
    key: &str,
    endpoint: &str,
) -> Result<Option<String>, ReadError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(unexpected(endpoint)),
    }
}

pub(crate) fn u64_at(body: &Value, key: &str, endpoint: &str) -> Result<u64, ReadError> {
    body.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| unexpected(endpoint))
}

/// The count at `total` in the summary object at `summary`, as GitHub's issue body carries
/// `sub_issues_summary.total` and `issue_dependencies_summary.total_blocked_by`.
///
/// A missing or `null` summary reads as 0, as [`text_at`] reads a missing body as empty:
/// older API responses omit the summaries. A summary that is present but has no count is
/// refused.
fn summary_total_at(
    body: &Value,
    summary: &str,
    total: &str,
    endpoint: &str,
) -> Result<u64, ReadError> {
    match body.get(summary) {
        None | Some(Value::Null) => Ok(0),
        Some(object) => u64_at(object, total, endpoint),
    }
}

/// The `login` of the user object at `key`.
pub(crate) fn login_at(body: &Value, key: &str, endpoint: &str) -> Result<Login, ReadError> {
    body.get(key)
        .map(|user| str_at(user, "login", endpoint))
        .unwrap_or_else(|| Err(unexpected(endpoint)))
        .map(Login)
}
