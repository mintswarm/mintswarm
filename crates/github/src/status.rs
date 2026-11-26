//! Reads and writes of GitHub issue statuses, design §9 "Reads and writes" and §13: an issue's
//! node id and status name come from the REST issue, and the repository's statuses and the
//! write go through GraphQL.
//!
//! A [`StatusClient`] is bound to one repository and keeps an ETag cache of its issues across
//! passes. A [`StatusPass`] is one pass with one token: each issue is read at most once, and the
//! repository's `planningStatuses` are queried once, by the first issue the pass needs them for.
//! GraphQL answers a refused request with a success status and an `errors` array: an error of
//! type `RATE_LIMITED` is a rate limit the retry layer waits out, retries and pauses for (see
//! `rest.rs`), and any other is a [`StatusError::GraphQl`], which is not retried.

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::StatusCode;
use serde_json::Value;
use tokio::sync::Mutex;

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::retry::{Exhausted, RetryPolicy};

use crate::etag::{ETAG_CACHE_LIMIT, EtagCache};
use crate::rest::{ClientError, GRAPHQL_PATH, Reply, Rest, check_names};
use crate::secret::Token;
use crate::token::Permissions;

/// The repository's issue statuses, by an issue of it.
const STATUSES_QUERY: &str = concat!(
    "query($id: ID!) { node(id: $id) { ... on Issue { repository {",
    " planningStatuses(first: 50) { nodes { id name } } } } } }",
);

/// Sets an issue's status to a status id.
const SET_STATUS: &str = concat!(
    "mutation($issue: ID!, $status: ID!) {",
    " setIssueStatus(input: {issueId: $issue, statusId: $status}) { clientMutationId } }",
);

/// Why a status read or write failed. No variant carries a token or a response body.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StatusError {
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
    /// The request kept failing transiently, rate limits included, until its retries ran out.
    #[error(transparent)]
    Exhausted(#[from] Exhausted),
    /// The answer is not JSON of the expected shape.
    #[error("{endpoint}: not the expected JSON")]
    Unexpected {
        /// The method and path.
        endpoint: String,
    },
    /// GraphQL refused the request with errors other than a rate limit.
    #[error("{endpoint}: GraphQL errors of types {types:?}")]
    GraphQl {
        /// The method and path.
        endpoint: String,
        /// The `type` of each error, in order; GitHub's messages are not kept.
        types: Vec<String>,
    },
    /// The repository has no status of this name; nothing was written.
    #[error("the repository has no issue status {name:?}")]
    UnknownStatus {
        /// The name asked for.
        name: String,
    },
}

/// A repository's issue statuses, as `(id, name)` pairs.
type Statuses = Arc<Vec<(String, String)>>;

/// Reads and writes issue statuses in one repository.
#[derive(Debug)]
pub struct StatusClient {
    rest: Rest,
    owner: String,
    repo: String,
    /// The last ETag and body of each issue, kept across passes.
    etags: Mutex<EtagCache<u64>>,
}

impl StatusClient {
    /// A client for `owner/repo` on the API at `api_base` (normally
    /// [`GITHUB_API`](crate::GITHUB_API)), retrying with the default [`RetryPolicy`] and
    /// pausing every later call after a rate limit that names a wait.
    pub fn new(api_base: &str, owner: &str, repo: &str) -> Result<Self, ClientError> {
        check_names(owner, repo)?;
        Ok(Self {
            rest: Rest::pausing(api_base, RetryPolicy::default())?,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            etags: Mutex::new(EtagCache::new(ETAG_CACHE_LIMIT)),
        })
    }

    /// The client retrying with `policy` in place of the default.
    #[must_use]
    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.rest = self.rest.with_policy(policy);
        self
    }

    /// The permissions a token for its passes asks for, from its
    /// [`TokenClient`](crate::TokenClient): writing issues, and reading the repository's metadata.
    pub fn permissions() -> Permissions {
        Permissions::write(["issues"]).and(Permissions::read(["metadata"]))
    }

    /// A pass over issues with `token`, which must allow reading issues, and writing them for
    /// [`StatusPass::set_status`].
    pub fn pass(&self, token: &Token) -> StatusPass<'_> {
        StatusPass {
            client: self,
            token: token.clone(),
            issues: Mutex::new(HashMap::new()),
            statuses: Mutex::new(None),
        }
    }

    fn issue_path(&self, issue: u64) -> String {
        format!("/repos/{}/{}/issues/{issue}", self.owner, self.repo)
    }
}

/// An issue's node id and status name, as the pass read them.
#[derive(Clone, Debug)]
struct IssueStatus {
    node_id: String,
    status: Option<String>,
}

/// One pass over issues: each issue is read at most once, and the repository's statuses are
/// queried at most once.
#[derive(Debug)]
pub struct StatusPass<'a> {
    client: &'a StatusClient,
    token: Token,
    issues: Mutex<HashMap<u64, IssueStatus>>,
    /// The repository's statuses as `(id, name)`, once queried.
    statuses: Mutex<Option<Statuses>>,
}

impl StatusPass<'_> {
    /// The name of `issue`'s status, or `None` when it has none.
    pub async fn read_status(&self, issue: IssueNumber) -> Result<Option<String>, StatusError> {
        Ok(self.issue(issue.0).await?.status)
    }

    /// Sets `issue`'s status to the repository's status named `name`, matched exactly.
    ///
    /// A name the repository has no status of is [`StatusError::UnknownStatus`], before the
    /// mutation is sent. Once the mutation was sent, whether or not it succeeded, a later read in
    /// the pass reads the issue again.
    pub async fn set_status(&self, issue: IssueNumber, name: &str) -> Result<(), StatusError> {
        let node_id = self.issue(issue.0).await?.node_id;
        let statuses = self.statuses(&node_id).await?;
        let Some((status_id, _)) = statuses.iter().find(|(_, known)| known == name) else {
            return Err(StatusError::UnknownStatus {
                name: name.to_owned(),
            });
        };
        let body = serde_json::json!({
            "query": SET_STATUS,
            "variables": {"issue": node_id, "status": status_id},
        });
        // Setting an issue's status to a status leaves it at that status however often it is
        // repeated.
        let sent = self
            .client
            .rest
            .post_idempotent(GRAPHQL_PATH, &self.token, &body)
            .await;
        // Whatever the outcome, the mutation may have changed the issue.
        self.forget(issue.0).await;
        graphql_data(&sent?)?;
        Ok(())
    }

    /// The pass's copy of `issue`'s node id and status, fetched once per pass and revalidated
    /// against the client's ETag cache.
    async fn issue(&self, issue: u64) -> Result<IssueStatus, StatusError> {
        let mut issues = self.issues.lock().await;
        if let Some(read) = issues.get(&issue) {
            return Ok(read.clone());
        }
        let cached = self.client.etags.lock().await.get(&issue);
        let path = self.client.issue_path(issue);
        let reply = self
            .client
            .rest
            .get(
                &path,
                &self.token,
                cached.as_ref().map(|(etag, _)| etag.as_str()),
            )
            .await?;
        let body = if reply.status == StatusCode::NOT_MODIFIED {
            let (_, body) = cached.ok_or_else(|| unexpected(&reply.endpoint))?;
            body
        } else if reply.status.is_success() {
            let body = Arc::new(json(&reply)?);
            let mut etags = self.client.etags.lock().await;
            match &reply.etag {
                Some(etag) => etags.insert(issue, etag.clone(), Arc::clone(&body)),
                None => etags.remove(&issue),
            }
            body
        } else {
            return Err(refused(&reply));
        };
        let read = map_issue_status(&body, &reply.endpoint)?;
        issues.insert(issue, read.clone());
        Ok(read)
    }

    /// The repository's statuses, queried by the issue `node_id` the first time a pass needs
    /// them.
    async fn statuses(&self, node_id: &str) -> Result<Statuses, StatusError> {
        let mut statuses = self.statuses.lock().await;
        if let Some(known) = statuses.as_ref() {
            return Ok(Arc::clone(known));
        }
        let body = serde_json::json!({
            "query": STATUSES_QUERY,
            "variables": {"id": node_id},
        });
        // A query changes nothing on GitHub.
        let reply = self
            .client
            .rest
            .post_idempotent(GRAPHQL_PATH, &self.token, &body)
            .await?;
        let data = graphql_data(&reply)?;
        let known = Arc::new(map_statuses(&data, &reply.endpoint)?);
        *statuses = Some(Arc::clone(&known));
        Ok(known)
    }

    /// Drops what this pass and the client hold for `issue`, after a write that may have changed
    /// it.
    async fn forget(&self, issue: u64) {
        self.issues.lock().await.remove(&issue);
        self.client.etags.lock().await.remove(&issue);
    }
}

/// The `node_id` and `status.name` of a REST issue; a `null` or absent status is none.
fn map_issue_status(body: &Value, endpoint: &str) -> Result<IssueStatus, StatusError> {
    let node_id = body
        .get("node_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| unexpected(endpoint))?
        .to_owned();
    let status = match body.get("status") {
        None | Some(Value::Null) => None,
        Some(status) => Some(
            status
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| unexpected(endpoint))?
                .to_owned(),
        ),
    };
    Ok(IssueStatus { node_id, status })
}

/// The `(id, name)` pairs of `data.node.repository.planningStatuses.nodes`.
fn map_statuses(data: &Value, endpoint: &str) -> Result<Vec<(String, String)>, StatusError> {
    data.pointer("/node/repository/planningStatuses/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| unexpected(endpoint))?
        .iter()
        .map(|status| {
            let text = |key| status.get(key).and_then(Value::as_str).map(str::to_owned);
            text("id")
                .zip(text("name"))
                .ok_or_else(|| unexpected(endpoint))
        })
        .collect()
}

/// A GraphQL reply's `data`: a status that is not a success, a body that is not JSON, a
/// non-empty `errors` or a `data` that is not an object is a failure.
fn graphql_data(reply: &Reply) -> Result<Value, StatusError> {
    if !reply.status.is_success() {
        return Err(refused(reply));
    }
    let mut body = json(reply)?;
    match body.get("errors") {
        None | Some(Value::Null) => {}
        Some(Value::Array(errors)) if errors.is_empty() => {}
        Some(Value::Array(errors)) => {
            return Err(StatusError::GraphQl {
                endpoint: reply.endpoint.clone(),
                types: errors
                    .iter()
                    .map(|error| {
                        error
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned()
                    })
                    .collect(),
            });
        }
        Some(_) => return Err(unexpected(&reply.endpoint)),
    }
    match body.get_mut("data").map(Value::take) {
        Some(data @ Value::Object(_)) => Ok(data),
        _ => Err(unexpected(&reply.endpoint)),
    }
}

/// The reply's body as JSON.
fn json(reply: &Reply) -> Result<Value, StatusError> {
    serde_json::from_slice(&reply.body).map_err(|_| unexpected(&reply.endpoint))
}

/// A refusal: [`StatusError::NotFound`] for a 404, [`StatusError::Status`] otherwise.
fn refused(reply: &Reply) -> StatusError {
    if reply.status == StatusCode::NOT_FOUND {
        StatusError::NotFound {
            endpoint: reply.endpoint.clone(),
        }
    } else {
        StatusError::Status {
            endpoint: reply.endpoint.clone(),
            status: reply.status.as_u16(),
        }
    }
}

fn unexpected(endpoint: &str) -> StatusError {
    StatusError::Unexpected {
        endpoint: endpoint.to_owned(),
    }
}
