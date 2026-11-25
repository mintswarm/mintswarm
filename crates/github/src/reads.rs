//! GET-only reads of one repository's issues, comments, links and artifacts as the runner App,
//! design §9 "Reads and writes".
//!
//! Every GET goes through one ETag cache, so a repeated read is revalidated with
//! `If-None-Match` and a 304 is served from the cache. Lists are read with `per_page=100`, page
//! by page, following the `Link` header's `rel="next"`, and a list with a page after its 30th
//! is refused. After a rate-limited reply with
//! `retry-after` the client sends nothing until that long has passed; the rate-limited read
//! itself fails with [`ReadError::RateLimited`] and is not retried.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::Value;

use mintswarm_domain::field::FieldError;
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{
    Artifact, ArtifactSummary, Comment, CommentMark, Cursor, Issue, IssueRead, IssueType, Link,
    LinkKind, Login,
};

use crate::etag::{ETAG_CACHE_LIMIT, EtagCache};
use crate::rest::{ClientError, Rest, check_names, path_segment};
use crate::secret::Token;
use crate::token::Permissions;

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
    /// GitHub refused the request for a rate limit; later requests wait for `retry_after`.
    #[error("{endpoint}: rate limited")]
    RateLimited {
        /// The method and path.
        endpoint: String,
        /// The `retry-after` GitHub gave, if any.
        retry_after: Option<Duration>,
    },
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
}

impl From<FieldError> for ReadError {
    fn from(error: FieldError) -> Self {
        match error {
            FieldError::Transport(message) => Self::Transport(message),
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

/// GET-only reads of one repository with a token its caller passes in.
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
    /// are the logins [`comment_mark`] is given.
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
            rest: Rest::pausing(api_base)?,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            kinds,
            trusted,
            bot,
            etags: Mutex::new(EtagCache::new(ETAG_CACHE_LIMIT)),
        })
    }

    /// The permissions a token for these reads asks for, from its
    /// [`TokenClient`](crate::TokenClient).
    pub fn permissions() -> Permissions {
        Permissions::read(["issues", "pull_requests", "checks", "statuses", "metadata"])
    }

    /// What is left of the pause a `retry-after` started; zero when there is none.
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
        };
        Ok((issue, str_at(body, "created_at", endpoint)?))
    }

    /// The JSON at `path`, revalidated against the ETag cache.
    pub(crate) async fn get(&self, token: &Token, path: &str) -> Result<Arc<Value>, ReadError> {
        Ok(self.get_page(token, path).await?.body)
    }

    /// The JSON at `path` with its next-page hint, revalidated against the ETag cache.
    async fn get_page(&self, token: &Token, path: &str) -> Result<Page, ReadError> {
        let cached = self.cache().get(&path.to_owned());
        let reply = self
            .rest
            .send(
                Method::GET,
                path,
                token,
                None,
                cached.as_ref().map(|(etag, _)| etag.as_str()),
            )
            .await?;
        if reply.status == StatusCode::NOT_MODIFIED {
            let (_, page) = cached.ok_or_else(|| {
                ReadError::Transport(format!("{}: 304 with nothing cached", reply.endpoint))
            })?;
            return Ok(page);
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
            return Ok(page);
        }
        Err(if reply.rate_limited {
            ReadError::RateLimited {
                endpoint: reply.endpoint,
                retry_after: reply.retry_after,
            }
        } else if reply.status == StatusCode::NOT_FOUND {
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
        let separator = if path.contains('?') { '&' } else { '?' };
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            let page_path = format!("{path}{separator}per_page={PER_PAGE}&page={page}");
            let page = self.get_page(token, &page_path).await?;
            let list = match items {
                Some(key) => page.body.get(key),
                None => Some(page.body.as_ref()),
            }
            .and_then(Value::as_array)
            .ok_or_else(|| unexpected(&format!("GET {page_path}")))?;
            all.extend(list.iter().cloned());
            if !page.next.unwrap_or(list.len() >= PER_PAGE) {
                return Ok(all);
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

fn map_comment(body: &Value, endpoint: &str) -> Result<Comment, ReadError> {
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

/// The `login` of the user object at `key`.
pub(crate) fn login_at(body: &Value, key: &str, endpoint: &str) -> Result<Login, ReadError> {
    body.get(key)
        .map(|user| str_at(user, "login", endpoint))
        .unwrap_or_else(|| Err(unexpected(endpoint)))
        .map(Login)
}
