//! The `github.webhook` normaliser and the sender classifier (design 5.3, 6.3, B4).
//!
//! [`normalise`] turns one verified GitHub delivery into a [`v1::WatchEvent`] whose body is a
//! complete [`v1::GitHubWebhook`], and hands the raw body back beside it for the store. Every
//! GitHub-written string travels as [`v1::UntrustedText`]; the summary is built only from
//! structured fields. [`Senders`] says who the sender is.

use std::collections::HashMap;

use mintswarm_config::Trust;
use mintswarm_domain::tracker::Login;
use mintswarm_proto::watch::v1;
use serde_json::{Map, Value};

use super::WatchError;

/// The kind of the events [`normalise`] builds.
pub const WEBHOOK_KIND: &str = "github.webhook";

/// The most bytes of GitHub-written text an [`v1::UntrustedText`] carries: 64 KiB.
pub const BODY_LIMIT: usize = 65_536;

/// One normalised delivery: the event, without `id` and `cursor`, which the store assigns, and
/// the verified raw body, unchanged, which the store keeps with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Normalised {
    /// The event; its `GitHubWebhook.payload` is empty.
    pub event: v1::WatchEvent,
    /// The raw body, byte for byte as it was given.
    pub payload: Vec<u8>,
}

/// Who the plane knows by name: the trusted authors, read from `tracker.ingest.trusted.authors`,
/// and the plane's own logins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Senders {
    maintainers: Vec<Login>,
    own: Vec<Login>,
}

impl Senders {
    /// The senders `trust` names as authors, and `self_logins`, the plane's own logins.
    pub fn new(trust: &Trust, self_logins: Vec<Login>) -> Self {
        Self {
            maintainers: trust.authors.clone(),
            own: self_logins,
        }
    }

    /// How `login` is classed: MAINTAINER for a trusted author, then SELF for one of the plane's
    /// own logins, then APP when `bot` is true, and PERSON otherwise.
    ///
    /// Logins are compared ASCII case-insensitively, as GitHub treats them.
    pub fn classify(&self, login: &str, bot: bool) -> v1::ActorClass {
        let named = |logins: &[Login]| logins.iter().any(|l| l.0.eq_ignore_ascii_case(login));
        if named(&self.maintainers) {
            v1::ActorClass::Maintainer
        } else if named(&self.own) {
            v1::ActorClass::Self_
        } else if bot {
            v1::ActorClass::App
        } else {
            v1::ActorClass::Person
        }
    }
}

/// `text` as untrusted text: cut at the last UTF-8 boundary at or below [`BODY_LIMIT`] bytes
/// when it is longer, with `original_length` its byte length, saturating at `u32::MAX`.
pub fn untrusted(text: &str) -> v1::UntrustedText {
    let mut end = text.len().min(BODY_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    v1::UntrustedText {
        text: text[..end].to_owned(),
        truncated: end < text.len(),
        original_length: u32::try_from(text.len()).unwrap_or(u32::MAX),
    }
}

/// Normalises the delivery `delivery` (X-GitHub-Delivery) of event `event` (X-GitHub-Event)
/// with the verified raw `body`, classing its sender with `senders`.
///
/// A field the payload omits leaves its output empty. Fails only for an empty `delivery` or
/// `event`, or a body that is not a JSON object.
pub fn normalise(
    delivery: &str,
    event: &str,
    body: Vec<u8>,
    senders: &Senders,
) -> Result<Normalised, WatchError> {
    if delivery.is_empty() {
        return Err(WatchError::Payload("the delivery id is empty".to_owned()));
    }
    if event.is_empty() {
        return Err(WatchError::Payload("the event name is empty".to_owned()));
    }
    let parsed: Value = serde_json::from_slice(&body)
        .map_err(|error| WatchError::Payload(format!("the body is not JSON: {error}")))?;
    let Some(payload) = parsed.as_object() else {
        return Err(WatchError::Payload(
            "the body is not a JSON object".to_owned(),
        ));
    };

    let action = text_at(payload.get("action"))
        .unwrap_or_default()
        .to_owned();
    let item = Item::of(payload);
    let subject = v1::Subject {
        repository: payload
            .get("repository")
            .and_then(|r| text_at(r.get("full_name")))
            .unwrap_or_default()
            .to_owned(),
        item_kind: item.kind.into(),
        number: item
            .object
            .and_then(|o| o.get("number")?.as_u64())
            .unwrap_or(0),
        url: item
            .object
            .and_then(|o| text_at(o.get("html_url")))
            .unwrap_or_default()
            .to_owned(),
        title: item
            .object
            .and_then(|o| text_at(o.get("title")))
            .map(untrusted),
        ..v1::Subject::default()
    };
    let actor = actor(payload.get("sender"), senders);
    let summary = summary(event, &action, &subject, &actor.login);

    let webhook = v1::GitHubWebhook {
        delivery: delivery.to_owned(),
        event: event.to_owned(),
        action: action.clone(),
        body: body_source(event, payload, item.object)
            .and_then(|source| text_at(source.get("body")))
            .map(untrusted),
        detail: detail(event, &action, payload),
        payload: Vec::new(),
    };

    let event = v1::WatchEvent {
        kind: WEBHOOK_KIND.to_owned(),
        parties: item.object.map(parties).unwrap_or_default(),
        attributes: HashMap::from([
            ("event".to_owned(), event.to_owned()),
            ("action".to_owned(), action),
        ]),
        subject: Some(subject),
        actor: Some(actor),
        summary,
        body: Some(v1::watch_event::Body::GithubWebhook(webhook)),
        ..v1::WatchEvent::default()
    };
    Ok(Normalised {
        event,
        payload: body,
    })
}

/// The item a payload is about, and its kind.
struct Item<'a> {
    object: Option<&'a Map<String, Value>>,
    kind: v1::ItemKind,
}

impl<'a> Item<'a> {
    /// `pull_request`, else `issue`: a pull request for the first and for an issue that has a
    /// `pull_request` key, otherwise an issue. With neither, no item and kind NONE.
    fn of(payload: &'a Map<String, Value>) -> Self {
        if let Some(object) = payload.get("pull_request").and_then(Value::as_object) {
            return Self {
                object: Some(object),
                kind: v1::ItemKind::PullRequest,
            };
        }
        match payload.get("issue").and_then(Value::as_object) {
            Some(object) => Self {
                object: Some(object),
                kind: if object.contains_key("pull_request") {
                    v1::ItemKind::PullRequest
                } else {
                    v1::ItemKind::Issue
                },
            },
            None => Self {
                object: None,
                kind: v1::ItemKind::None,
            },
        }
    }
}

/// The string at `value`, when it is one.
fn text_at(value: Option<&Value>) -> Option<&str> {
    value?.as_str()
}

/// The sender as an actor: its login and id, BOT for a `Bot` type or a `[bot]` login, and its
/// class.
fn actor(sender: Option<&Value>, senders: &Senders) -> v1::Actor {
    let login = sender
        .and_then(|s| text_at(s.get("login")))
        .unwrap_or_default();
    let id = match sender.and_then(|s| s.get("id")) {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let bot =
        sender.and_then(|s| text_at(s.get("type"))) == Some("Bot") || login.ends_with("[bot]");
    v1::Actor {
        login: login.to_owned(),
        id,
        r#type: if bot {
            v1::ActorType::Bot
        } else {
            v1::ActorType::User
        }
        .into(),
        class: senders.classify(login, bot).into(),
    }
}

/// The item's author, every assignee and every requested reviewer.
fn parties(item: &Map<String, Value>) -> Vec<v1::Party> {
    let party = |user: &Value, role: &str| {
        text_at(user.get("login")).map(|login| v1::Party {
            login: login.to_owned(),
            role: role.to_owned(),
        })
    };
    let all = |key: &str, role: &'static str| {
        item.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(move |user| party(user, role))
    };
    item.get("user")
        .and_then(|user| party(user, "author"))
        .into_iter()
        .chain(all("assignees", "assignee"))
        .chain(all("requested_reviewers", "requested_reviewer"))
        .collect()
}

/// The object whose `body` is the event's body, one per event family, with no fallback: the
/// comment, the review, or the item for `issues` and `pull_request`.
fn body_source<'a>(
    event: &str,
    payload: &'a Map<String, Value>,
    item: Option<&'a Map<String, Value>>,
) -> Option<&'a Map<String, Value>> {
    match event {
        "issue_comment" | "pull_request_review_comment" => {
            payload.get("comment").and_then(Value::as_object)
        }
        "pull_request_review" => payload.get("review").and_then(Value::as_object),
        "issues" | "pull_request" => item,
        _ => None,
    }
}

/// The four detail values, each set only when the payload has it: `review_state`, `merged`,
/// `label` and `check_conclusion`.
fn detail(
    event: &str,
    action: &str,
    payload: &Map<String, Value>,
) -> HashMap<String, v1::UntrustedText> {
    let mut detail = HashMap::new();
    let mut set = |key: &str, value: Option<&str>| {
        if let Some(value) = value {
            detail.insert(key.to_owned(), untrusted(value));
        }
    };
    if event == "pull_request_review" {
        set(
            "review_state",
            payload.get("review").and_then(|r| text_at(r.get("state"))),
        );
    }
    if event == "pull_request" && action == "closed" {
        let merged = payload
            .get("pull_request")
            .and_then(|pr| pr.get("merged")?.as_bool());
        set("merged", merged.map(|m| if m { "true" } else { "false" }));
    }
    if action == "labeled" || action == "unlabeled" {
        set(
            "label",
            payload.get("label").and_then(|l| text_at(l.get("name"))),
        );
    }
    if matches!(event, "check_suite" | "check_run") && action == "completed" {
        set(
            "check_conclusion",
            payload
                .get(event)
                .and_then(|c| text_at(c.get("conclusion"))),
        );
    }
    detail
}

/// `github.webhook <event>[.<action>] <repository>[#<number>] by <login>`, from structured
/// fields only, each through [`identifier`].
fn summary(event: &str, action: &str, subject: &v1::Subject, login: &str) -> String {
    let mut line = format!("{WEBHOOK_KIND} {}", identifier(event));
    if !action.is_empty() {
        line.push('.');
        line.push_str(identifier(action));
    }
    if !subject.repository.is_empty() {
        line.push(' ');
        line.push_str(identifier(&subject.repository));
    }
    if subject.number != 0 {
        line.push_str(&format!("#{}", subject.number));
    }
    if !login.is_empty() {
        line.push_str(" by ");
        line.push_str(identifier(login));
    }
    line
}

/// `value` when every character is ASCII alphanumeric or one of `-_./[]`, and `?` otherwise, so
/// no free text reaches the summary.
fn identifier(value: &str) -> &str {
    let allowed = |c: char| c.is_ascii_alphanumeric() || "-_./[]".contains(c);
    if value.chars().all(allowed) {
        value
    } else {
        "?"
    }
}
