//! The pull request reads: the pull, its reviews, review comments, review threads and requested
//! reviewers, the files it changes, and the check runs, check suites and statuses on a commit.
//!
//! [`GitHubReads::read_pr`] reads the pull with its reviews and checks in one record; the other
//! reads each read one resource, so a caller can re-read just that one before it acts. Review
//! threads are read through GraphQL, since REST does not say whether a thread is resolved.

use serde_json::Value;

use mintswarm_domain::ids::{PrNumber, RevId};
use mintswarm_domain::tracker::{
    ChangedFile, CheckRun, CheckState, CheckSuite, CombinedStatus, Login, PullDetail, PullRequest,
    RequestedReviewers, Review, ReviewComment, ReviewThread,
};
use mintswarm_domain::vcs::BranchName;

use crate::reads::{
    GitHubReads, GraphQlReply, MAX_PAGES, PER_PAGE, ReadError, graphql_reply, login_at, opt_str_at,
    str_at, text_at, u64_at, unexpected,
};
use crate::rest::{GRAPHQL_PATH, Reply, path_segment};
use crate::secret::Token;

/// A pull request's review threads, a page of 100 at a time after a cursor.
const THREADS_QUERY: &str = concat!(
    "query($owner: String!, $name: String!, $number: Int!, $after: String) {",
    " repository(owner: $owner, name: $name) { pullRequest(number: $number) {",
    " reviewThreads(first: 100, after: $after) {",
    " nodes { id isResolved } pageInfo { hasNextPage endCursor } } } } }",
);

/// The most files GitHub lists for one pull request.
const MAX_PR_FILES: usize = 3000;

// The paged read must be able to reach GitHub's limit, or a cut-off list would come back short.
const _: () = assert!(MAX_PAGES as usize * PER_PAGE >= MAX_PR_FILES);

impl GitHubReads {
    /// Pull request `pr`: its head, state, mergeable state and requested reviewers, its
    /// reviews, and the latest check runs and the statuses on its head.
    pub async fn read_pr(&self, token: &Token, pr: PrNumber) -> Result<PullRequest, ReadError> {
        let path = self.pull_path(&pr);
        let endpoint = format!("GET {path}");
        let pull = self.get(token, &path).await?;
        let head = pull.get("head").ok_or_else(|| unexpected(&endpoint))?;
        let base = pull.get("base").ok_or_else(|| unexpected(&endpoint))?;
        let sha = str_at(head, "sha", &endpoint)?;
        let requested_reviewers = pull
            .get("requested_reviewers")
            .and_then(Value::as_array)
            .ok_or_else(|| unexpected(&endpoint))?
            .iter()
            .map(|user| str_at(user, "login", &endpoint).map(Login))
            .collect::<Result<Vec<_>, _>>()?;

        let reviews = self.read_reviews(token, pr).await?;

        let commit = self.commit_path(&sha);
        let runs_path = format!("{commit}/check-runs?filter=latest");
        let runs_endpoint = format!("GET {runs_path}");
        let mut checks = self
            .get_paged(token, &runs_path, Some("check_runs"))
            .await?
            .iter()
            .map(|run| map_check_state(run, &runs_endpoint))
            .collect::<Result<Vec<_>, _>>()?;
        let status_path = format!("{commit}/status");
        let status_endpoint = format!("GET {status_path}");
        for status in self
            .get_paged(token, &status_path, Some("statuses"))
            .await?
        {
            checks.push(map_status(&status, &status_endpoint)?);
        }

        Ok(PullRequest {
            number: PrNumber(u64_at(&pull, "number", &endpoint)?),
            url: str_at(&pull, "html_url", &endpoint)?,
            branch: BranchName(str_at(head, "ref", &endpoint)?),
            base: BranchName(str_at(base, "ref", &endpoint)?),
            head: RevId(sha),
            title: str_at(&pull, "title", &endpoint)?,
            state: str_at(&pull, "state", &endpoint)?,
            draft: pull
                .get("draft")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
            mergeable_state: opt_str_at(&pull, "mergeable_state", &endpoint)?,
            reviews,
            requested_reviewers,
            checks,
        })
    }

    /// Pull request `pr`'s own fields: its head and base, state, labels, whether it can be
    /// merged or was, and how many files it changes.
    pub async fn read_pull(&self, token: &Token, pr: PrNumber) -> Result<PullDetail, ReadError> {
        let path = self.pull_path(&pr);
        let pull = self.get(token, &path).await?;
        map_pull(&pull, &format!("GET {path}"))
    }

    /// Every review of pull request `pr`, in GitHub's order, with the commit each was given on.
    pub async fn read_reviews(
        &self,
        token: &Token,
        pr: PrNumber,
    ) -> Result<Vec<Review>, ReadError> {
        let path = format!("{}/reviews", self.pull_path(&pr));
        let endpoint = format!("GET {path}");
        self.get_paged(token, &path, None)
            .await?
            .iter()
            .map(|review| map_review(review, &endpoint))
            .collect()
    }

    /// Every comment on pull request `pr`'s diff.
    pub async fn read_review_comments(
        &self,
        token: &Token,
        pr: PrNumber,
    ) -> Result<Vec<ReviewComment>, ReadError> {
        let path = format!("{}/comments", self.pull_path(&pr));
        let endpoint = format!("GET {path}");
        self.get_paged(token, &path, None)
            .await?
            .iter()
            .map(|comment| map_review_comment(comment, &endpoint))
            .collect()
    }

    /// The people and teams a review of pull request `pr` is requested from.
    pub async fn read_requested_reviewers(
        &self,
        token: &Token,
        pr: PrNumber,
    ) -> Result<RequestedReviewers, ReadError> {
        let path = format!("{}/requested_reviewers", self.pull_path(&pr));
        let endpoint = format!("GET {path}");
        let body = self.get(token, &path).await?;
        let list = |key| {
            body.get(key)
                .and_then(Value::as_array)
                .ok_or_else(|| unexpected(&endpoint))
        };
        Ok(RequestedReviewers {
            users: list("users")?
                .iter()
                .map(|user| str_at(user, "login", &endpoint).map(Login))
                .collect::<Result<_, _>>()?,
            teams: list("teams")?
                .iter()
                .map(|team| str_at(team, "slug", &endpoint))
                .collect::<Result<_, _>>()?,
        })
    }

    /// Every review thread of pull request `pr` and whether it is resolved, through GraphQL,
    /// following `endCursor` page by page. A thread list that goes on after page [`MAX_PAGES`]
    /// is [`ReadError::TooManyPages`], a reply with `errors` is [`ReadError::GraphQl`], and one
    /// without the repository or the pull request is [`ReadError::NotFound`].
    pub async fn read_review_threads(
        &self,
        token: &Token,
        pr: PrNumber,
    ) -> Result<Vec<ReviewThread>, ReadError> {
        let mut threads = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let body = serde_json::json!({
                "query": THREADS_QUERY,
                "variables": {
                    "owner": self.owner(),
                    "name": self.repo(),
                    "number": pr.0,
                    "after": after,
                },
            });
            // A query changes nothing on GitHub.
            let reply = self
                .rest
                .post_idempotent(GRAPHQL_PATH, token, &body)
                .await?;
            let data = graphql_data(&reply)?;
            let endpoint = &reply.endpoint;
            let page = match data.pointer("/repository/pullRequest") {
                Some(Value::Object(_)) => data
                    .pointer("/repository/pullRequest/reviewThreads")
                    .ok_or_else(|| unexpected(endpoint))?,
                None | Some(Value::Null) => {
                    return Err(ReadError::NotFound {
                        endpoint: endpoint.clone(),
                    });
                }
                Some(_) => return Err(unexpected(endpoint)),
            };
            for node in page
                .get("nodes")
                .and_then(Value::as_array)
                .ok_or_else(|| unexpected(endpoint))?
            {
                threads.push(ReviewThread {
                    id: str_at(node, "id", endpoint)?,
                    resolved: node
                        .get("isResolved")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| unexpected(endpoint))?,
                });
            }
            let info = page.get("pageInfo").ok_or_else(|| unexpected(endpoint))?;
            let more = info
                .get("hasNextPage")
                .and_then(Value::as_bool)
                .ok_or_else(|| unexpected(endpoint))?;
            if !more {
                return Ok(threads);
            }
            after = Some(str_at(info, "endCursor", endpoint)?);
        }
        Err(ReadError::TooManyPages {
            endpoint: format!("POST {GRAPHQL_PATH}"),
        })
    }

    /// The latest check run of each name on commit `sha`.
    pub async fn read_check_runs(
        &self,
        token: &Token,
        sha: &RevId,
    ) -> Result<Vec<CheckRun>, ReadError> {
        let path = format!("{}/check-runs?filter=latest", self.commit_path(&sha.0));
        let endpoint = format!("GET {path}");
        self.get_paged(token, &path, Some("check_runs"))
            .await?
            .iter()
            .map(|run| map_check_run(run, &endpoint))
            .collect()
    }

    /// The check suites on commit `sha`.
    pub async fn read_check_suites(
        &self,
        token: &Token,
        sha: &RevId,
    ) -> Result<Vec<CheckSuite>, ReadError> {
        let path = format!("{}/check-suites", self.commit_path(&sha.0));
        let endpoint = format!("GET {path}");
        self.get_paged(token, &path, Some("check_suites"))
            .await?
            .iter()
            .map(|suite| map_check_suite(suite, &endpoint))
            .collect()
    }

    /// The combined commit status of commit `sha`: its state and count from the first page,
    /// and the latest status of each context from every page.
    pub async fn read_statuses(
        &self,
        token: &Token,
        sha: &RevId,
    ) -> Result<CombinedStatus, ReadError> {
        let path = format!("{}/status", self.commit_path(&sha.0));
        let endpoint = format!("GET {path}");
        let paged = self.get_paged_first(token, &path, Some("statuses")).await?;
        Ok(CombinedStatus {
            state: str_at(&paged.first, "state", &endpoint)?,
            total_count: u64_at(&paged.first, "total_count", &endpoint)?,
            statuses: paged
                .items
                .iter()
                .map(|status| map_status(status, &endpoint))
                .collect::<Result<_, _>>()?,
        })
    }

    /// Every file pull request `pr` changes, with the previous path of a rename.
    ///
    /// GitHub lists at most [`MAX_PR_FILES`] files, so a list that reaches that many is
    /// [`ReadError::TooManyPages`]: it may be cut off, and is never returned short.
    pub async fn read_changed_files(
        &self,
        token: &Token,
        pr: PrNumber,
    ) -> Result<Vec<ChangedFile>, ReadError> {
        let path = format!("{}/files", self.pull_path(&pr));
        let endpoint = format!("GET {path}");
        let files = self.get_paged(token, &path, None).await?;
        if files.len() >= MAX_PR_FILES {
            return Err(ReadError::TooManyPages { endpoint });
        }
        files
            .iter()
            .map(|file| map_changed_file(file, &endpoint))
            .collect()
    }

    fn pull_path(&self, pr: &PrNumber) -> String {
        format!("{}/pulls/{}", self.repo_path(), pr.0)
    }

    fn commit_path(&self, sha: &str) -> String {
        format!("{}/commits/{}", self.repo_path(), path_segment(sha))
    }
}

/// Whether the user object `user` is a bot: of type `Bot`, or with a login ending in `[bot]`.
fn is_bot(user: &Value) -> bool {
    user.get("type").and_then(Value::as_str) == Some("Bot")
        || user
            .get("login")
            .and_then(Value::as_str)
            .is_some_and(|login| login.ends_with("[bot]"))
}

/// Whether the author at `key` is a bot; a missing author is not.
fn author_is_bot(body: &Value, key: &str) -> bool {
    body.get(key).is_some_and(is_bot)
}

fn map_pull(body: &Value, endpoint: &str) -> Result<PullDetail, ReadError> {
    let bad = || unexpected(endpoint);
    let head = body.get("head").ok_or_else(bad)?;
    let base = body.get("base").ok_or_else(bad)?;
    let head_repo = match head.get("repo") {
        None | Some(Value::Null) => None,
        Some(repo) => Some(str_at(repo, "full_name", endpoint)?),
    };
    let labels = body
        .get("labels")
        .and_then(Value::as_array)
        .ok_or_else(bad)?
        .iter()
        .map(|label| str_at(label, "name", endpoint))
        .collect::<Result<_, _>>()?;
    let mergeable = match body.get("mergeable") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(mergeable)) => Some(*mergeable),
        Some(_) => return Err(bad()),
    };
    Ok(PullDetail {
        number: PrNumber(u64_at(body, "number", endpoint)?),
        url: str_at(body, "html_url", endpoint)?,
        title: str_at(body, "title", endpoint)?,
        state: str_at(body, "state", endpoint)?,
        draft: body
            .get("draft")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        base: BranchName(str_at(base, "ref", endpoint)?),
        branch: BranchName(str_at(head, "ref", endpoint)?),
        head: RevId(str_at(head, "sha", endpoint)?),
        head_repo,
        labels,
        mergeable,
        mergeable_state: opt_str_at(body, "mergeable_state", endpoint)?,
        merged: body
            .get("merged")
            .and_then(Value::as_bool)
            .ok_or_else(bad)?,
        merge_commit: opt_str_at(body, "merge_commit_sha", endpoint)?.map(RevId),
        changed_files: u64_at(body, "changed_files", endpoint)?,
    })
}

fn map_review(body: &Value, endpoint: &str) -> Result<Review, ReadError> {
    Ok(Review {
        id: u64_at(body, "id", endpoint)?,
        author: login_at(body, "user", endpoint)?,
        state: str_at(body, "state", endpoint)?,
        submitted_at: opt_str_at(body, "submitted_at", endpoint)?,
        commit: opt_str_at(body, "commit_id", endpoint)?.map(RevId),
        url: text_at(body, "html_url", endpoint)?,
        author_is_bot: author_is_bot(body, "user"),
    })
}

fn map_review_comment(body: &Value, endpoint: &str) -> Result<ReviewComment, ReadError> {
    let review = match body.get("pull_request_review_id") {
        None | Some(Value::Null) => None,
        Some(id) => Some(id.as_u64().ok_or_else(|| unexpected(endpoint))?),
    };
    Ok(ReviewComment {
        id: u64_at(body, "id", endpoint)?,
        review,
        author: login_at(body, "user", endpoint)?,
        author_is_bot: author_is_bot(body, "user"),
        body: text_at(body, "body", endpoint)?,
        created_at: str_at(body, "created_at", endpoint)?,
        updated_at: str_at(body, "updated_at", endpoint)?,
        url: str_at(body, "html_url", endpoint)?,
    })
}

/// The slug of the app object at `key`; `None` for a missing or `null` app.
fn app_slug(body: &Value, endpoint: &str) -> Result<Option<String>, ReadError> {
    match body.get("app") {
        None | Some(Value::Null) => Ok(None),
        Some(app) => opt_str_at(app, "slug", endpoint),
    }
}

/// A check run as [`read_pr`](GitHubReads::read_pr) lists it, beside the statuses.
fn map_check_state(body: &Value, endpoint: &str) -> Result<CheckState, ReadError> {
    Ok(CheckState {
        name: str_at(body, "name", endpoint)?,
        conclusion: opt_str_at(body, "conclusion", endpoint)?,
        source: String::from("check-run"),
        app: app_slug(body, endpoint)?,
    })
}

fn map_check_run(body: &Value, endpoint: &str) -> Result<CheckRun, ReadError> {
    let suite = match body.get("check_suite") {
        None | Some(Value::Null) => None,
        Some(suite) => Some(u64_at(suite, "id", endpoint)?),
    };
    Ok(CheckRun {
        id: u64_at(body, "id", endpoint)?,
        name: str_at(body, "name", endpoint)?,
        status: str_at(body, "status", endpoint)?,
        conclusion: opt_str_at(body, "conclusion", endpoint)?,
        app: app_slug(body, endpoint)?,
        suite,
    })
}

fn map_check_suite(body: &Value, endpoint: &str) -> Result<CheckSuite, ReadError> {
    Ok(CheckSuite {
        id: u64_at(body, "id", endpoint)?,
        app: app_slug(body, endpoint)?,
        status: opt_str_at(body, "status", endpoint)?,
        conclusion: opt_str_at(body, "conclusion", endpoint)?,
        latest_check_runs_count: u64_at(body, "latest_check_runs_count", endpoint)?,
    })
}

fn map_status(body: &Value, endpoint: &str) -> Result<CheckState, ReadError> {
    Ok(CheckState {
        name: str_at(body, "context", endpoint)?,
        // A pending status has not concluded, like a check run whose conclusion is null.
        conclusion: opt_str_at(body, "state", endpoint)?.filter(|state| state != "pending"),
        source: String::from("status"),
        app: None,
    })
}

fn map_changed_file(body: &Value, endpoint: &str) -> Result<ChangedFile, ReadError> {
    Ok(ChangedFile {
        path: str_at(body, "filename", endpoint)?,
        previous_path: opt_str_at(body, "previous_filename", endpoint)?,
        status: str_at(body, "status", endpoint)?,
    })
}

/// A GraphQL reply's `data`: a 404 is [`ReadError::NotFound`], another status that is not a
/// success [`ReadError::Status`], a non-empty `errors` [`ReadError::GraphQl`] with only the
/// errors' types, and a body that is not JSON or a `data` that is not an object
/// [`ReadError::Unexpected`].
fn graphql_data(reply: &Reply) -> Result<Value, ReadError> {
    let endpoint = &reply.endpoint;
    match graphql_reply(reply) {
        GraphQlReply::Data(data) => Ok(data),
        GraphQlReply::Refused if reply.status == reqwest::StatusCode::NOT_FOUND => {
            Err(ReadError::NotFound {
                endpoint: endpoint.clone(),
            })
        }
        GraphQlReply::Refused => Err(ReadError::Status {
            endpoint: endpoint.clone(),
            status: reply.status.as_u16(),
        }),
        GraphQlReply::Errors(types) => Err(ReadError::GraphQl {
            endpoint: endpoint.clone(),
            types,
        }),
        GraphQlReply::Unexpected => Err(unexpected(endpoint)),
    }
}
