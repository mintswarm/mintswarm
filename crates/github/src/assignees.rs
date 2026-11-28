//! An issue's assignees and the `assigned` events that made them, and the rule that tells the
//! assignments the plane's own logins made from those a person made.
//!
//! [`GitHubReads::read_assignees`] reads the issue's `assignees`, and
//! [`GitHubReads::read_assigned_events`] pages `issues/{n}/events` and keeps the `assigned`
//! ones. [`releasable`] takes both and the plane's own logins, and names the assignees whose
//! latest `assigned` event came from one of those logins or through the App one of them acts as.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::Login;

use crate::reads::{GitHubReads, ReadError, str_at, unexpected};
use crate::secret::Token;

/// The suffix GitHub gives an App's bot login after its slug.
const BOT_SUFFIX: &str = "[bot]";

/// One `assigned` event of an issue: who was assigned, and by whom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignedEvent {
    /// The login assigned.
    pub assignee: Login,
    /// The login that acted, if the event names one.
    pub actor: Option<Login>,
    /// The login that made the assignment, if the event names one.
    pub assigner: Option<Login>,
    /// The slug of the App the assignment was made through, if any.
    pub app: Option<String>,
    /// When, in RFC 3339, if the event says.
    pub created_at: Option<String>,
    /// The event's id, if the event has one.
    pub id: Option<u64>,
}

impl GitHubReads {
    /// The logins `issue` is assigned to, as its `assignees` list them; none when the issue has
    /// no `assignees`.
    pub async fn read_assignees(
        &self,
        token: &Token,
        issue: IssueNumber,
    ) -> Result<Vec<Login>, ReadError> {
        let path = format!("{}/issues/{}", self.repo_path(), issue.0);
        let body = self.get(token, &path).await?;
        assignee_logins(&body, &format!("GET {path}"))
    }

    /// The `assigned` events of `issue`, in the order GitHub lists them, paged. Events of other
    /// kinds, and `assigned` events that name no assignee login, are left out. A list of more
    /// than [`MAX_PAGES`](crate::MAX_PAGES) pages is [`ReadError::TooManyPages`].
    pub async fn read_assigned_events(
        &self,
        token: &Token,
        issue: IssueNumber,
    ) -> Result<Vec<AssignedEvent>, ReadError> {
        let path = format!("{}/issues/{}/events", self.repo_path(), issue.0);
        Ok(self
            .get_paged(token, &path, None)
            .await?
            .iter()
            .filter_map(assigned_event)
            .collect())
    }
}

/// The logins in `body`'s `assignees`, an issue as GitHub answers it; none for a missing or
/// `null` list.
pub(crate) fn assignee_logins(body: &Value, endpoint: &str) -> Result<Vec<Login>, ReadError> {
    match body.get("assignees") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(users)) => users
            .iter()
            .map(|user| str_at(user, "login", endpoint).map(Login))
            .collect(),
        Some(_) => Err(unexpected(endpoint)),
    }
}

/// The login of the user object at `key`, if it names one.
fn login_of(body: &Value, key: &str) -> Option<Login> {
    body.get(key)?
        .get("login")?
        .as_str()
        .map(|login| Login(login.to_owned()))
}

/// `event` as an [`AssignedEvent`], when it is an `assigned` event naming its assignee's login.
fn assigned_event(event: &Value) -> Option<AssignedEvent> {
    if event.get("event").and_then(Value::as_str) != Some("assigned") {
        return None;
    }
    Some(AssignedEvent {
        assignee: login_of(event, "assignee")?,
        actor: login_of(event, "actor"),
        assigner: login_of(event, "assigner"),
        app: event
            .get("performed_via_github_app")
            .and_then(|app| app.get("slug"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        created_at: event
            .get("created_at")
            .and_then(Value::as_str)
            .map(str::to_owned),
        id: event.get("id").and_then(Value::as_u64),
    })
}

/// The assignees of `assignees` to release: those whose latest `assigned` event, by
/// `created_at` and then `id` (an event without one sorts first), was made by one of `own`, as
/// its actor or its assigner, or through the App whose slug an own login ending in `[bot]` names
/// before that suffix. An own login without `[bot]` names no App. Logins and slugs are compared
/// ASCII case-insensitively. An assignee without an `assigned` event, or whose latest one a
/// person made, is kept. The result is sorted and has no repeats.
pub fn releasable(assignees: &[Login], events: &[AssignedEvent], own: &[Login]) -> Vec<Login> {
    let own_logins: HashSet<String> = own
        .iter()
        .map(|login| login.0.to_ascii_lowercase())
        .collect();
    let own_slugs: HashSet<String> = own
        .iter()
        .filter_map(|login| {
            let lower = login.0.to_ascii_lowercase();
            lower.strip_suffix(BOT_SUFFIX).map(str::to_owned)
        })
        .collect();
    let mut ordered: Vec<&AssignedEvent> = events.iter().collect();
    ordered.sort_by(|a, b| {
        let key = |event: &AssignedEvent| {
            (
                event.created_at.clone().unwrap_or_default(),
                event.id.unwrap_or(0),
            )
        };
        key(a).cmp(&key(b))
    });
    let mut latest: HashMap<String, &AssignedEvent> = HashMap::new();
    for event in ordered {
        latest.insert(event.assignee.0.to_ascii_lowercase(), event);
    }
    let is_own = |login: &Option<Login>| {
        login
            .as_ref()
            .is_some_and(|login| own_logins.contains(&login.0.to_ascii_lowercase()))
    };
    let mut released: Vec<Login> = assignees
        .iter()
        .filter(|assignee| {
            latest
                .get(&assignee.0.to_ascii_lowercase())
                .is_some_and(|event| {
                    is_own(&event.actor)
                        || is_own(&event.assigner)
                        || event
                            .app
                            .as_ref()
                            .is_some_and(|slug| own_slugs.contains(&slug.to_ascii_lowercase()))
                })
        })
        .cloned()
        .collect();
    released.sort();
    released.dedup();
    released
}
