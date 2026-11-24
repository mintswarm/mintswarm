//! The options of a role's field on GitHub, for the operator's admin commands (design §4.6). It
//! uses the token its caller gives it, never one the runner mints. Fields are configured by
//! name, so it also resolves a field's id from its name.

use reqwest::{Method, StatusCode};
use serde_json::Value;

use mintswarm_domain::field::{
    FieldCatalogue, FieldError, FieldFinding, FieldRef, FieldSource, FieldValue,
};

use crate::fields::{Op, Pattern, classify};
use crate::rest::{ClientError, Reply, Rest, check_names};
use crate::secret::Token;

/// Labels are listed this many to a page.
const PER_PAGE: usize = 100;
/// At most this many pages of labels are read.
const MAX_PAGES: usize = 100;

/// Lists a field's options in one repository's organisation with the operator's token.
#[derive(Debug)]
pub struct GitHubFieldCatalogue {
    rest: Rest,
    owner: String,
    repo: String,
    token: Token,
}

impl GitHubFieldCatalogue {
    /// A catalogue for `owner/repo` on the API at `api_base`, calling with `token`.
    pub fn new(api_base: &str, owner: &str, repo: &str, token: Token) -> Result<Self, ClientError> {
        check_names(owner, repo)?;
        Ok(Self {
            rest: Rest::new(api_base)?,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            token,
        })
    }

    async fn get(&self, path: &str) -> Result<Reply, FieldError> {
        self.rest
            .send(Method::GET, path, &self.token, None, None)
            .await
    }

    /// The id of the organisation's issue field named `name`, or `field_missing` when the
    /// organisation has no field of that name.
    pub async fn field_id(&self, name: &str) -> Result<u64, FieldError> {
        for page in 1..=MAX_PAGES {
            let path = format!(
                "/orgs/{}/issue-fields?per_page={PER_PAGE}&page={page}",
                self.owner
            );
            let bad = || FieldError::Transport(format!("GET {path}: not the expected JSON"));
            let body = success(&self.get(&path).await?)?;
            let fields = body.as_array().ok_or_else(bad)?;
            if let Some(field) = fields
                .iter()
                .find(|field| field.get("name").and_then(Value::as_str) == Some(name))
            {
                return field.get("id").and_then(Value::as_u64).ok_or_else(bad);
            }
            if fields.len() < PER_PAGE {
                break;
            }
        }
        Err(FieldError::Field(FieldFinding::FieldMissing))
    }

    /// Every option of the organisation's issue field `field_id`, mapped or not.
    pub async fn field_options(&self, field_id: u64) -> Result<Vec<FieldValue>, FieldError> {
        let path = format!("/orgs/{}/issue-fields/{field_id}", self.owner);
        let bad = || FieldError::Transport(format!("GET {path}: not the expected JSON"));
        let reply = self.get(&path).await?;
        // The resource is the field itself, so a plain 404 means the field is missing.
        if reply.status == StatusCode::NOT_FOUND {
            return Err(FieldError::Field(FieldFinding::FieldMissing));
        }
        success(&reply)?
            .get("options")
            .and_then(Value::as_array)
            .ok_or_else(bad)?
            .iter()
            .map(|option| {
                Some(FieldValue {
                    field_id: Some(field_id),
                    option_id: Some(option.get("id")?.as_u64()?),
                    name: option.get("name")?.as_str()?.to_owned(),
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(bad)
    }

    async fn label_options(&self, pattern: &str) -> Result<Vec<FieldValue>, FieldError> {
        let pattern = Pattern::checked(pattern)?;
        let mut options = Vec::new();
        for page in 1..=MAX_PAGES {
            let path = format!(
                "/repos/{}/{}/labels?per_page={PER_PAGE}&page={page}",
                self.owner, self.repo
            );
            let body = success(&self.get(&path).await?)?;
            let labels = body.as_array().ok_or_else(|| {
                FieldError::Transport(format!("GET {path}: not the expected JSON"))
            })?;
            for label in labels {
                let name = label
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(option) = pattern.option_of(name) {
                    options.push(FieldValue {
                        field_id: None,
                        option_id: None,
                        name: option.to_owned(),
                    });
                }
            }
            if labels.len() < PER_PAGE {
                break;
            }
        }
        Ok(options)
    }
}

/// The body of a successful reply, or the reply classified as a read's error.
fn success(reply: &Reply) -> Result<Value, FieldError> {
    if reply.status.is_success() {
        reply.json()
    } else {
        Err(classify(reply, Op::Read))
    }
}

impl FieldCatalogue for GitHubFieldCatalogue {
    async fn options(&self, field: &FieldRef) -> Result<Vec<FieldValue>, FieldError> {
        match &field.source {
            FieldSource::Field { name } => self.field_options(self.field_id(name).await?).await,
            FieldSource::Labels { label } => self.label_options(label).await,
        }
    }
}
