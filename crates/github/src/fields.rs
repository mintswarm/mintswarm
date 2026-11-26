//! Reads and writes of a role's field on GitHub issues, design §4.3 and §4.6: organisation
//! issue fields by id, or one label per option.
//!
//! A field is configured by name only; the caller resolves its id (for example with
//! [`GitHubFieldCatalogue::field_id`](crate::GitHubFieldCatalogue::field_id)) and passes it to
//! each read and write, and resolves it again when an error calls for it.
//!
//! A [`FieldClient`] is bound to one repository and keeps an ETag cache across passes. A
//! [`FieldPass`] is one pass over issues with one token: every role of an issue is read from one
//! response per resource, and the next pass revalidates it with `If-None-Match`. Field-class
//! refusals become [`FieldError::Field`] and every other refusal [`FieldError::Transport`], in
//! [`classify`] only; transient failures are the retry layer's, and end in
//! [`FieldError::Exhausted`] when its retries run out.

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::StatusCode;
use serde_json::Value;
use tokio::sync::Mutex;

use mintswarm_domain::field::{
    FieldError, FieldFinding, FieldRead, FieldRef, FieldSource, FieldValue,
};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::retry::RetryPolicy;

use crate::etag::{ETAG_CACHE_LIMIT, EtagCache};
use crate::rest::{ClientError, Reply, Rest, check_names, error_text, path_segment};
use crate::secret::Token;
use crate::token::Permissions;

/// The per-issue resources a pass reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Resource {
    /// `issues/{n}/issue-field-values`.
    FieldValues,
    /// `issues/{n}`, for its labels.
    Issue,
}

type Key = (u64, Resource);

/// Reads and writes role fields on the issues of one repository.
#[derive(Debug)]
pub struct FieldClient {
    rest: Rest,
    owner: String,
    repo: String,
    /// The last ETag and body of each issue's resources, kept across passes.
    etags: Mutex<EtagCache<Key>>,
}

impl FieldClient {
    /// A client for `owner/repo` on the API at `api_base` (normally
    /// [`GITHUB_API`](crate::GITHUB_API)), retrying with the default [`RetryPolicy`].
    pub fn new(api_base: &str, owner: &str, repo: &str) -> Result<Self, ClientError> {
        check_names(owner, repo)?;
        Ok(Self {
            rest: Rest::new(api_base, RetryPolicy::default())?,
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

    /// A pass over issues with `token`, which must allow what the pass does: reading issues and
    /// their fields, and writing them for [`FieldPass::set_field`].
    pub fn pass(&self, token: &Token) -> FieldPass<'_> {
        FieldPass {
            client: self,
            token: token.clone(),
            memo: Mutex::new(HashMap::new()),
        }
    }

    fn issue_path(&self, issue: u64) -> String {
        format!("/repos/{}/{}/issues/{issue}", self.owner, self.repo)
    }

    fn path(&self, issue: u64, resource: Resource) -> String {
        match resource {
            Resource::FieldValues => format!("{}/issue-field-values", self.issue_path(issue)),
            Resource::Issue => self.issue_path(issue),
        }
    }
}

/// One pass over issues: each issue's resources are fetched at most once, and every read of the
/// pass is served from that response.
#[derive(Debug)]
pub struct FieldPass<'a> {
    client: &'a FieldClient,
    token: Token,
    memo: Mutex<HashMap<Key, Arc<Value>>>,
}

impl FieldPass<'_> {
    /// The value of the role's field on `issue`.
    ///
    /// `field_id` is the id the caller resolved from the field's name; labels mode ignores it,
    /// and a field-mode read without one is `field_missing`, before any request.
    /// A field-mode value whose field has the mapped name but another id is
    /// `field_recreated`; [`FieldError::should_resolve_again`] tells the caller to resolve the
    /// id again by name.
    pub async fn read_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
        field_id: Option<u64>,
    ) -> Result<FieldRead, FieldError> {
        match &field.source {
            FieldSource::Field { name } => {
                let field_id = resolved(field_id)?;
                let entries = self.fetch(issue.0, Resource::FieldValues).await?;
                let endpoint = format!("GET {}", self.client.path(issue.0, Resource::FieldValues));
                resolve_values(&entries, name, field_id, &field.options, &endpoint)
            }
            FieldSource::Labels { label } => {
                let pattern = checked_pattern(label)?;
                let labels = self.labels(issue.0).await?;
                resolve_labels(&labels, &pattern, &field.options)
            }
        }
    }

    /// Sets the role's field on `issue` to the option `option`, or clears it for `None`.
    ///
    /// `field_id` is the id the caller resolved from the field's name, as for
    /// [`read_field`](Self::read_field).
    /// An option the map does not list is refused with `option_unmapped` before any request,
    /// as is a label pattern without `{option}` with `field_missing`.
    /// A write GitHub refuses, or answers with another value, is `write_rejected`, and one whose
    /// answer shows the mapped name under another id is `field_recreated`; both call for
    /// resolving the id again ([`FieldError::should_resolve_again`]).
    pub async fn set_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
        field_id: Option<u64>,
        option: Option<&str>,
    ) -> Result<(), FieldError> {
        if let Some(name) = option
            && !field.options.iter().any(|known| known == name)
        {
            return Err(FieldError::Field(FieldFinding::OptionUnmapped));
        }
        match &field.source {
            FieldSource::Field { name } => {
                let field_id = resolved(field_id)?;
                self.write_value(issue.0, name, field_id, &field.options, option)
                    .await?;
            }
            FieldSource::Labels { label } => {
                let pattern = checked_pattern(label)?;
                self.write_labels(issue.0, &pattern, option).await?;
            }
        }
        self.forget(issue.0).await;
        Ok(())
    }

    async fn write_value(
        &self,
        issue: u64,
        name: &str,
        field_id: u64,
        options: &[String],
        option: Option<&str>,
    ) -> Result<(), FieldError> {
        let path = self.client.path(issue, Resource::FieldValues);
        let Some(value) = option else {
            let reply = self
                .client
                .rest
                .delete(&clear_path(&path, field_id), &self.token)
                .await?;
            // A 404 after an attempt that may have landed is that attempt's clear.
            let gone = reply.status == StatusCode::NOT_FOUND && reply.after_ambiguous;
            return if reply.status.is_success() || gone {
                Ok(())
            } else {
                Err(classify(&reply, Op::Write))
            };
        };
        let body = serde_json::json!({
            "issue_field_values": [{"field_id": field_id, "value": value}],
        });
        // Setting a field to a value leaves it at that value however often it is repeated.
        let reply = self
            .client
            .rest
            .post_idempotent(&path, &self.token, &body)
            .await?;
        if !reply.status.is_success() {
            return Err(classify(&reply, Op::Write));
        }
        // Whatever the answer holds other than the value written, recreated field aside, is a
        // rejected write, so the caller resolves the field again.
        match resolve_values(&reply.json()?, name, field_id, options, &reply.endpoint) {
            Ok(FieldRead::Value(got)) if got.name == value => Ok(()),
            Err(
                error @ (FieldError::Transport(_)
                | FieldError::Exhausted(_)
                | FieldError::Field(FieldFinding::FieldRecreated)),
            ) => Err(error),
            _ => Err(FieldError::Field(FieldFinding::WriteRejected)),
        }
    }

    async fn write_labels(
        &self,
        issue: u64,
        pattern: &Pattern<'_>,
        option: Option<&str>,
    ) -> Result<(), FieldError> {
        // Only the role's labels are touched, so labels others changed since the pass's copy
        // was fetched stay as they are. The target is added before the others are removed: a
        // write cut short leaves a conflict to see, never a lost value.
        let mut present = false;
        let mut stale = Vec::new();
        for label in self.labels(issue).await? {
            let Some(found) = pattern.option_of(&label) else {
                continue;
            };
            if !present && option.is_some_and(|option| option.eq_ignore_ascii_case(found)) {
                present = true;
            } else {
                stale.push(label);
            }
        }
        let labels_path = format!("{}/labels", self.client.issue_path(issue));
        if let Some(option) = option.filter(|_| !present) {
            let body = serde_json::json!({ "labels": [pattern.label(option)] });
            // Adding a label the issue already has leaves its labels as they are, so a repeat
            // adds nothing.
            let reply = self
                .client
                .rest
                .post_idempotent(&labels_path, &self.token, &body)
                .await?;
            if !reply.status.is_success() {
                return Err(classify(&reply, Op::Write));
            }
        }
        for label in stale {
            let path = format!("{labels_path}/{}", path_segment(&label));
            let reply = self.client.rest.delete(&path, &self.token).await?;
            // A label someone else removed meanwhile, or an earlier attempt that may have landed
            // removed, is already where the write wants it.
            let gone = reply.status == StatusCode::NOT_FOUND
                && (reply.after_ambiguous
                    || error_text(&reply.body).contains("label does not exist"));
            if !reply.status.is_success() && !gone {
                return Err(classify(&reply, Op::Write));
            }
        }
        Ok(())
    }

    /// The names of `issue`'s labels, from the pass's copy of the issue.
    async fn labels(&self, issue: u64) -> Result<Vec<String>, FieldError> {
        let body = self.fetch(issue, Resource::Issue).await?;
        let bad = || {
            FieldError::Transport(format!(
                "GET {}: not the expected JSON",
                self.client.path(issue, Resource::Issue)
            ))
        };
        body.get("labels")
            .and_then(Value::as_array)
            .ok_or_else(bad)?
            .iter()
            .map(|label| match label {
                Value::String(name) => Some(name.clone()),
                other => other.get("name").and_then(Value::as_str).map(str::to_owned),
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(bad)
    }

    /// The pass's copy of an issue's resource: fetched once per pass, revalidated against the
    /// client's ETag cache.
    async fn fetch(&self, issue: u64, resource: Resource) -> Result<Arc<Value>, FieldError> {
        let key = (issue, resource);
        let mut memo = self.memo.lock().await;
        if let Some(body) = memo.get(&key) {
            return Ok(Arc::clone(body));
        }
        let cached = self.client.etags.lock().await.get(&key);
        let path = self.client.path(issue, resource);
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
            let (_, body) = cached.ok_or_else(|| {
                FieldError::Transport(format!("{}: 304 with nothing cached", reply.endpoint))
            })?;
            body
        } else if reply.status.is_success() {
            let body = Arc::new(reply.json()?);
            let mut etags = self.client.etags.lock().await;
            match reply.etag {
                Some(etag) => etags.insert(key, etag, Arc::clone(&body)),
                None => etags.remove(&key),
            }
            body
        } else {
            return Err(classify(&reply, Op::Read));
        };
        memo.insert(key, Arc::clone(&body));
        Ok(body)
    }

    /// Drops what this pass and the client hold for `issue`, after a write changed it.
    async fn forget(&self, issue: u64) {
        let mut memo = self.memo.lock().await;
        let mut etags = self.client.etags.lock().await;
        for resource in [Resource::FieldValues, Resource::Issue] {
            memo.remove(&(issue, resource));
            etags.remove(&(issue, resource));
        }
    }
}

/// The resolved id of a field-mode role, or `field_missing` when the caller has none, so it
/// resolves the id by the field's name ([`FieldError::should_resolve_again`]).
fn resolved(field_id: Option<u64>) -> Result<u64, FieldError> {
    field_id.ok_or(FieldError::Field(FieldFinding::FieldMissing))
}

/// The path a DELETE clears a field on an issue at, given the issue's `issue-field-values` path.
fn clear_path(values_path: &str, field_id: u64) -> String {
    format!("{values_path}/{field_id}")
}

/// Resolves a field against an issue's `issue-field-values` entries: the entry with the field's
/// id gives the value; failing that, an entry with the field's name but another id means the
/// field was recreated.
fn resolve_values(
    entries: &Value,
    name: &str,
    field_id: u64,
    options: &[String],
    endpoint: &str,
) -> Result<FieldRead, FieldError> {
    let entries = entries
        .as_array()
        .ok_or_else(|| FieldError::Transport(format!("{endpoint}: not the expected JSON")))?;
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.get("issue_field_id").and_then(Value::as_u64) == Some(field_id))
    {
        let option = entry.get("single_select_option");
        let value = option
            .and_then(|option| option.get("name"))
            .and_then(Value::as_str)
            .or_else(|| entry.get("value").and_then(Value::as_str))
            .unwrap_or_default();
        if value.is_empty() {
            return Ok(FieldRead::Unset);
        }
        if !options.iter().any(|known| known == value) {
            return Err(FieldError::Field(FieldFinding::OptionUnmapped));
        }
        return Ok(FieldRead::Value(FieldValue {
            field_id: Some(field_id),
            option_id: option
                .and_then(|option| option.get("id"))
                .and_then(Value::as_u64),
            name: value.to_owned(),
        }));
    }
    if entries
        .iter()
        .any(|entry| entry.get("issue_field_name").and_then(Value::as_str) == Some(name))
    {
        return Err(FieldError::Field(FieldFinding::FieldRecreated));
    }
    Ok(FieldRead::Unset)
}

/// Resolves a labels-mode role against an issue's labels.
fn resolve_labels(
    labels: &[String],
    pattern: &Pattern<'_>,
    options: &[String],
) -> Result<FieldRead, FieldError> {
    let mut values: Vec<(FieldValue, bool)> = labels
        .iter()
        .filter_map(|label| pattern.option_of(label))
        .map(|found| {
            let mapped = options
                .iter()
                .find(|known| known.eq_ignore_ascii_case(found));
            let name = mapped.map_or(found, String::as_str).to_owned();
            (
                FieldValue {
                    field_id: None,
                    option_id: None,
                    name,
                },
                mapped.is_some(),
            )
        })
        .collect();
    match values.len() {
        0 => Ok(FieldRead::Unset),
        1 => match values.pop() {
            Some((value, true)) => Ok(FieldRead::Value(value)),
            _ => Err(FieldError::Field(FieldFinding::OptionUnmapped)),
        },
        _ => Ok(FieldRead::Conflict(
            values.into_iter().map(|(value, _)| value).collect(),
        )),
    }
}

/// The domain's labels-mode matcher, shared so the edge and [`read_labels`] agree on R3.
///
/// [`read_labels`]: mintswarm_domain::field::read_labels
pub(crate) use mintswarm_domain::field::LabelPattern as Pattern;

/// The pattern's parts, or `field_missing` when it does not hold `{option}` exactly once: such a
/// pattern names no option, so the role maps no field.
pub(crate) fn checked_pattern(pattern: &str) -> Result<Pattern<'_>, FieldError> {
    Pattern::new(pattern).ok_or(FieldError::Field(FieldFinding::FieldMissing))
}

/// Whether a request read or wrote, for [`classify`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Read,
    Write,
}

/// The error for a reply that is not a success: the only place statuses become errors. Rate
/// limits and server errors never get here, as the retry layer retries them and ends in
/// [`FieldError::Exhausted`]; any such reply would be [`FieldError::Transport`].
pub(crate) fn classify(reply: &Reply, op: Op) -> FieldError {
    let transport = || FieldError::Transport(format!("{}: {}", reply.endpoint, reply.status));
    let text = error_text(&reply.body);
    match reply.status.as_u16() {
        404 | 422 if text.contains("option") => FieldError::Field(FieldFinding::OptionMissing),
        404 | 422 if text.contains("field") => FieldError::Field(FieldFinding::FieldMissing),
        422 if op == Op::Write => FieldError::Field(FieldFinding::WriteRejected),
        403 => FieldError::Field(match op {
            Op::Write => FieldFinding::WriteRejected,
            Op::Read => FieldFinding::FieldMissing,
        }),
        _ => transport(),
    }
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use mintswarm_domain::field::{FieldError, FieldFinding};

    use std::sync::Arc;

    use super::{Op, Pattern, Resource, checked_pattern, classify};
    use crate::etag::EtagCache;
    use crate::rest::Reply;

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            endpoint: String::from("POST /repos/o/r/issues/7/issue-field-values"),
            status: StatusCode::from_u16(status).unwrap(),
            etag: None,
            rate_limited: false,
            retry_after: None,
            reset_wait: None,
            next_page: None,
            date: None,
            body: body.as_bytes().to_vec(),
            after_ambiguous: false,
        }
    }

    fn field(finding: FieldFinding) -> FieldError {
        FieldError::Field(finding)
    }

    fn is_transport(error: &FieldError) -> bool {
        matches!(error, FieldError::Transport(_))
    }

    #[test]
    fn server_errors_that_reach_it_are_transport() {
        // The retry layer keeps them away; one that got here would still not be drift.
        for op in [Op::Read, Op::Write] {
            for status in [500, 502, 503] {
                assert!(is_transport(&classify(&reply(status, ""), op)));
            }
        }
        let error = classify(&reply(502, ""), Op::Write);
        assert_eq!(
            error.to_string(),
            "transport error: POST /repos/o/r/issues/7/issue-field-values: 502 Bad Gateway"
        );
    }

    #[test]
    fn options_are_checked_before_fields() {
        let body = r#"{"message":"Validation Failed","errors":[
            {"field":"value","message":"Option 'XXL' does not exist for field Effort"}]}"#;
        assert_eq!(
            classify(&reply(422, body), Op::Write),
            field(FieldFinding::OptionMissing)
        );
        assert_eq!(
            classify(&reply(404, r#"{"message":"Field not found"}"#), Op::Read),
            field(FieldFinding::FieldMissing)
        );
        let body = r#"{"message":"Validation Failed","errors":[{"field":"field_id"}]}"#;
        assert_eq!(
            classify(&reply(422, body), Op::Write),
            field(FieldFinding::FieldMissing)
        );
    }

    #[test]
    fn other_refusals_split_by_status_and_operation() {
        let body = r#"{"message":"Validation Failed"}"#;
        assert_eq!(
            classify(&reply(422, body), Op::Write),
            field(FieldFinding::WriteRejected)
        );
        assert!(is_transport(&classify(&reply(422, body), Op::Read)));
        let forbidden = r#"{"message":"Resource not accessible by integration"}"#;
        assert_eq!(
            classify(&reply(403, forbidden), Op::Write),
            field(FieldFinding::WriteRejected)
        );
        assert_eq!(
            classify(&reply(403, forbidden), Op::Read),
            field(FieldFinding::FieldMissing)
        );
        for op in [Op::Read, Op::Write] {
            assert!(is_transport(&classify(
                &reply(404, r#"{"message":"Not Found"}"#),
                op
            )));
            assert!(is_transport(&classify(&reply(401, ""), op)));
            assert!(is_transport(&classify(&reply(409, "not json"), op)));
        }
    }

    #[test]
    fn the_etag_cache_drops_its_oldest_entry_when_full() {
        let mut cache = EtagCache::new(2);
        let body = Arc::new(serde_json::Value::Null);
        cache.insert((1, Resource::Issue), "a".into(), Arc::clone(&body));
        cache.insert((2, Resource::Issue), "b".into(), Arc::clone(&body));
        // Storing a key again refreshes it without dropping another.
        cache.insert((1, Resource::Issue), "a2".into(), Arc::clone(&body));
        assert_eq!(cache.len(), 2);
        cache.insert((3, Resource::FieldValues), "c".into(), body);
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&(2, Resource::Issue)).is_none());
        assert_eq!(cache.get(&(1, Resource::Issue)).unwrap().0, "a2");
        assert_eq!(cache.get(&(3, Resource::FieldValues)).unwrap().0, "c");
    }

    #[test]
    fn patterns_match_case_insensitively_around_the_option() {
        let pattern = Pattern::new("effort:{option}").unwrap();
        assert_eq!(pattern.option_of("effort:S"), Some("S"));
        assert_eq!(pattern.option_of("Effort:XL"), Some("XL"));
        assert_eq!(pattern.option_of("effort:"), None);
        assert_eq!(pattern.option_of("priority:S"), None);
        assert_eq!(pattern.option_of("eff"), None);

        let pattern = Pattern::new("[{option}] size").unwrap();
        assert_eq!(pattern.option_of("[M] Size"), Some("M"));
        assert_eq!(pattern.option_of("[M] size!"), None);
        assert_eq!(pattern.option_of("[] size"), None);

        let pattern = Pattern::new("é:{option}").unwrap();
        assert_eq!(pattern.option_of("é:S"), Some("S"));
        assert_eq!(pattern.option_of("ab:S"), None);

        assert!(Pattern::new("effort").is_none());
        assert_eq!(
            checked_pattern("effort").unwrap_err(),
            FieldError::Field(FieldFinding::FieldMissing)
        );
        assert_eq!(
            checked_pattern("[{option}] size").unwrap().label("M"),
            "[M] size"
        );
    }
}
