//! The pull request read: the pull, its reviews, and the check runs and statuses on its head.

use serde_json::Value;

use mintswarm_domain::ids::{PrNumber, RevId};
use mintswarm_domain::tracker::{CheckState, Login, PullRequest, Review};
use mintswarm_domain::vcs::BranchName;

use crate::fields::path_segment;
use crate::reads::{
    GitHubReads, ReadError, login_at, opt_str_at, str_at, text_at, u64_at, unexpected,
};
use crate::secret::Token;

impl GitHubReads {
    /// Pull request `pr`: its head, state, mergeable state and requested reviewers, its
    /// reviews, and the latest check runs and the statuses on its head.
    pub async fn read_pr(&self, token: &Token, pr: PrNumber) -> Result<PullRequest, ReadError> {
        let path = format!("{}/pulls/{}", self.repo_path(), pr.0);
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

        let reviews_path = format!("{path}/reviews");
        let reviews_endpoint = format!("GET {reviews_path}");
        let reviews = self
            .get_paged(token, &reviews_path, None)
            .await?
            .iter()
            .map(|review| map_review(review, &reviews_endpoint))
            .collect::<Result<Vec<_>, _>>()?;

        let commit = format!("{}/commits/{}", self.repo_path(), path_segment(&sha));
        let runs_path = format!("{commit}/check-runs?filter=latest");
        let runs_endpoint = format!("GET {runs_path}");
        let mut checks = self
            .get_paged(token, &runs_path, Some("check_runs"))
            .await?
            .iter()
            .map(|run| map_check_run(run, &runs_endpoint))
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
}

fn map_review(body: &Value, endpoint: &str) -> Result<Review, ReadError> {
    Ok(Review {
        id: u64_at(body, "id", endpoint)?,
        author: login_at(body, "user", endpoint)?,
        state: str_at(body, "state", endpoint)?,
        submitted_at: opt_str_at(body, "submitted_at", endpoint)?,
        commit: opt_str_at(body, "commit_id", endpoint)?.map(RevId),
        url: text_at(body, "html_url", endpoint)?,
    })
}

fn map_check_run(body: &Value, endpoint: &str) -> Result<CheckState, ReadError> {
    let app = match body.get("app") {
        None | Some(Value::Null) => None,
        Some(app) => opt_str_at(app, "slug", endpoint)?,
    };
    Ok(CheckState {
        name: str_at(body, "name", endpoint)?,
        conclusion: opt_str_at(body, "conclusion", endpoint)?,
        source: String::from("check-run"),
        app,
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
