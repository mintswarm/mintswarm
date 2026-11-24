//! Reads and writes of a role's field on GitHub issues, design §4.3 and §4.6: organisation
//! issue fields by id, or one label per option.
//!
//! A [`FieldClient`] is bound to one repository and keeps an ETag cache across passes. A
//! [`FieldPass`] is one pass over issues with one token: every role of an issue is read from one
//! response per resource, and the next pass revalidates it with `If-None-Match`. Field-class
//! refusals become [`FieldError::Field`] and everything else [`FieldError::Transport`], in
//! [`classify`] only.

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::{Method, StatusCode};
use serde_json::Value;
use tokio::sync::Mutex;

use mintswarm_domain::field::{
    FieldError, FieldFinding, FieldRead, FieldRef, FieldSource, FieldValue,
};
use mintswarm_domain::ids::IssueNumber;

use crate::rest::{ClientError, Reply, Rest, check_names};
use crate::secret::Token;

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
    etags: Mutex<HashMap<Key, (String, Arc<Value>)>>,
}

impl FieldClient {
    /// A client for `owner/repo` on the API at `api_base` (normally
    /// [`GITHUB_API`](crate::GITHUB_API)).
    pub fn new(api_base: &str, owner: &str, repo: &str) -> Result<Self, ClientError> {
        check_names(owner, repo)?;
        Ok(Self {
            rest: Rest::new(api_base)?,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            etags: Mutex::new(HashMap::new()),
        })
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
    /// A field-mode value whose field has the mapped name but another id is
    /// `field_recreated`; [`FieldError::should_resolve_again`] tells the caller to resolve the
    /// id again by name.
    pub async fn read_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
    ) -> Result<FieldRead, FieldError> {
        match &field.source {
            FieldSource::Field { name, field_id } => {
                let entries = self.fetch(issue.0, Resource::FieldValues).await?;
                let endpoint = format!("GET {}", self.client.path(issue.0, Resource::FieldValues));
                resolve_values(&entries, name, *field_id, &field.options, &endpoint)
            }
            FieldSource::Labels { label } => {
                let labels = self.labels(issue.0).await?;
                resolve_labels(&labels, label, &field.options)
            }
        }
    }

    /// Sets the role's field on `issue` to the option `option`, or clears it for `None`.
    ///
    /// An option the map does not list is refused with `option_unmapped` before any request.
    /// A write GitHub refuses, or answers with another value, is `write_rejected`, and one whose
    /// answer shows the mapped name under another id is `field_recreated`; both call for
    /// resolving the id again ([`FieldError::should_resolve_again`]).
    pub async fn set_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
        option: Option<&str>,
    ) -> Result<(), FieldError> {
        if let Some(name) = option {
            if !field.options.iter().any(|known| known == name) {
                return Err(FieldError::Field(FieldFinding::OptionUnmapped));
            }
        }
        match &field.source {
            FieldSource::Field { name, field_id } => {
                self.write_value(issue.0, name, *field_id, &field.options, option)
                    .await?;
            }
            FieldSource::Labels { label } => {
                self.write_labels(issue.0, label, option).await?;
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
            let (method, path) = clear_request(&path, field_id);
            let reply = self.send(method, &path, None).await?;
            return if reply.status.is_success() {
                Ok(())
            } else {
                Err(classify(&reply, Op::Write))
            };
        };
        let body = serde_json::json!({
            "issue_field_values": [{"field_id": field_id, "value": value}],
        });
        let reply = self.send(Method::POST, &path, Some(body)).await?;
        if !reply.status.is_success() {
            return Err(classify(&reply, Op::Write));
        }
        // Whatever the answer holds other than the value written, recreated field aside, is a
        // rejected write, so the caller resolves the field again.
        match resolve_values(&reply.json()?, name, field_id, options, &reply.endpoint) {
            Ok(FieldRead::Value(got)) if got.name == value => Ok(()),
            Err(
                error
                @ (FieldError::Transport(_) | FieldError::Field(FieldFinding::FieldRecreated)),
            ) => Err(error),
            _ => Err(FieldError::Field(FieldFinding::WriteRejected)),
        }
    }

    async fn write_labels(
        &self,
        issue: u64,
        pattern: &str,
        option: Option<&str>,
    ) -> Result<(), FieldError> {
        let pattern_parts = Pattern::new(pattern);
        let mut labels: Vec<String> = self
            .labels(issue)
            .await?
            .into_iter()
            .filter(|label| {
                pattern_parts
                    .as_ref()
                    .is_none_or(|pattern| pattern.option_of(label).is_none())
            })
            .collect();
        if let Some(option) = option {
            labels.push(pattern.replace(OPTION, option));
        }
        let path = format!("{}/labels", self.client.issue_path(issue));
        let body = serde_json::json!({ "labels": labels });
        let reply = self.send(Method::PUT, &path, Some(body)).await?;
        if reply.status.is_success() {
            Ok(())
        } else {
            Err(classify(&reply, Op::Write))
        }
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
        let cached = self.client.etags.lock().await.get(&key).cloned();
        let path = self.client.path(issue, resource);
        let reply = self
            .client
            .rest
            .send(
                Method::GET,
                &path,
                &self.token,
                None,
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
                Some(etag) => etags.insert(key, (etag, Arc::clone(&body))),
                None => etags.remove(&key),
            };
            body
        } else {
            return Err(classify(&reply, Op::Read));
        };
        memo.insert(key, Arc::clone(&body));
        Ok(body)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Reply, FieldError> {
        self.client
            .rest
            .send(method, path, &self.token, body, None)
            .await
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

/// The request that clears a field on an issue, given the issue's `issue-field-values` path.
fn clear_request(values_path: &str, field_id: u64) -> (Method, String) {
    (Method::DELETE, format!("{values_path}/{field_id}"))
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
    pattern: &str,
    options: &[String],
) -> Result<FieldRead, FieldError> {
    let Some(pattern) = Pattern::new(pattern) else {
        return Ok(FieldRead::Unset);
    };
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

/// The placeholder for the option's name in a label pattern.
pub(crate) const OPTION: &str = "{option}";

/// A label pattern split at its `{option}` placeholder.
#[derive(Debug)]
pub(crate) struct Pattern<'a> {
    prefix: &'a str,
    suffix: &'a str,
}

impl<'a> Pattern<'a> {
    /// The pattern's parts; `None` when it has no `{option}`, so it matches no label.
    pub(crate) fn new(pattern: &'a str) -> Option<Self> {
        let (prefix, suffix) = pattern.split_once(OPTION)?;
        Some(Self { prefix, suffix })
    }

    /// The option `label` names under this pattern, compared case-insensitively.
    pub(crate) fn option_of<'l>(&self, label: &'l str) -> Option<&'l str> {
        let rest = label.len().checked_sub(self.suffix.len())?;
        let head = label.get(..self.prefix.len())?;
        let tail = label.get(rest..)?;
        let option = label.get(self.prefix.len()..rest)?;
        (head.eq_ignore_ascii_case(self.prefix)
            && tail.eq_ignore_ascii_case(self.suffix)
            && !option.is_empty())
        .then_some(option)
    }
}

/// Whether a request read or wrote, for [`classify`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Read,
    Write,
}

/// The error for a reply that is not a success: the only place statuses become errors.
pub(crate) fn classify(reply: &Reply, op: Op) -> FieldError {
    let transport = || FieldError::Transport(format!("{}: {}", reply.endpoint, reply.status));
    let text = error_text(&reply.body);
    let status = reply.status;
    if reply.rate_limited
        || status.is_server_error()
        || (matches!(status.as_u16(), 403 | 429) && text.contains("rate limit"))
    {
        return transport();
    }
    match status.as_u16() {
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

/// The lower-cased `message`, `errors[].message` and `errors[].field` of an error body.
fn error_text(body: &[u8]) -> String {
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(message) = body.get("message").and_then(Value::as_str) {
        parts.push(message);
    }
    for error in body
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for key in ["message", "field"] {
            if let Some(text) = error.get(key).and_then(Value::as_str) {
                parts.push(text);
            }
        }
    }
    parts.join("\n").to_lowercase()
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use mintswarm_domain::field::{FieldError, FieldFinding};

    use super::{Op, Pattern, classify};
    use crate::rest::Reply;

    fn reply(status: u16, rate_limited: bool, body: &str) -> Reply {
        Reply {
            endpoint: String::from("POST /repos/o/r/issues/7/issue-field-values"),
            status: StatusCode::from_u16(status).unwrap(),
            etag: None,
            rate_limited,
            body: body.as_bytes().to_vec(),
        }
    }

    fn field(finding: FieldFinding) -> FieldError {
        FieldError::Field(finding)
    }

    fn is_transport(error: &FieldError) -> bool {
        matches!(error, FieldError::Transport(_))
    }

    #[test]
    fn rate_limits_and_server_errors_are_transport() {
        for op in [Op::Read, Op::Write] {
            assert!(is_transport(&classify(&reply(429, true, ""), op)));
            assert!(is_transport(&classify(&reply(403, true, ""), op)));
            assert!(is_transport(&classify(
                &reply(
                    403,
                    false,
                    r#"{"message":"You have exceeded a secondary rate limit"}"#
                ),
                op
            )));
            for status in [500, 502, 503] {
                assert!(is_transport(&classify(&reply(status, false, ""), op)));
            }
        }
        let error = classify(&reply(502, false, ""), Op::Write);
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
            classify(&reply(422, false, body), Op::Write),
            field(FieldFinding::OptionMissing)
        );
        assert_eq!(
            classify(
                &reply(404, false, r#"{"message":"Field not found"}"#),
                Op::Read
            ),
            field(FieldFinding::FieldMissing)
        );
        let body = r#"{"message":"Validation Failed","errors":[{"field":"field_id"}]}"#;
        assert_eq!(
            classify(&reply(422, false, body), Op::Write),
            field(FieldFinding::FieldMissing)
        );
    }

    #[test]
    fn other_refusals_split_by_status_and_operation() {
        let body = r#"{"message":"Validation Failed"}"#;
        assert_eq!(
            classify(&reply(422, false, body), Op::Write),
            field(FieldFinding::WriteRejected)
        );
        assert!(is_transport(&classify(&reply(422, false, body), Op::Read)));
        let forbidden = r#"{"message":"Resource not accessible by integration"}"#;
        assert_eq!(
            classify(&reply(403, false, forbidden), Op::Write),
            field(FieldFinding::WriteRejected)
        );
        assert_eq!(
            classify(&reply(403, false, forbidden), Op::Read),
            field(FieldFinding::FieldMissing)
        );
        for op in [Op::Read, Op::Write] {
            assert!(is_transport(&classify(
                &reply(404, false, r#"{"message":"Not Found"}"#),
                op
            )));
            assert!(is_transport(&classify(&reply(401, false, ""), op)));
            assert!(is_transport(&classify(&reply(409, false, "not json"), op)));
        }
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
    }
}
