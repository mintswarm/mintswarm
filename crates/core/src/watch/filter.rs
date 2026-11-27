//! The filter evaluator: whether a `WatchEvent` matches a validated `WatchFilter`.

use mintswarm_proto::watch::v1::{ActorClass, Verdict, WatchEvent, WatchFilter};

use super::WatchError;

/// A `WatchFilter` that passed validation. Fields are ANDed and the values within a field
/// ORed; an empty field matches everything, except `kinds`, which must not be empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter(WatchFilter);

impl Filter {
    /// Validates `filter`; fails for an empty `kinds` or any malformed value.
    pub fn new(filter: WatchFilter) -> Result<Self, WatchError> {
        validate(&filter)?;
        Ok(Self(filter))
    }

    /// The filter as the client sent it.
    pub fn as_proto(&self) -> &WatchFilter {
        &self.0
    }

    /// Whether `event` matches every field of the filter and its verdict lets it through.
    pub fn matches(&self, event: &WatchEvent) -> bool {
        let f = &self.0;
        let (repository, number) = event
            .subject
            .as_ref()
            .map_or(("", 0), |s| (s.repository.as_str(), s.number));
        let (login, class) = event
            .actor
            .as_ref()
            .map_or(("", 0), |a| (a.login.as_str(), a.class));
        f.kinds.iter().any(|k| kind_matches(k, &event.kind))
            && any_or_empty(&f.repositories, |r| r.eq_ignore_ascii_case(repository))
            && any_or_empty(&f.items, |i| {
                i.number == number && i.repository.eq_ignore_ascii_case(repository)
            })
            && any_or_empty(&f.actors, |a| a.eq_ignore_ascii_case(login))
            && any_or_empty(&f.actor_classes, |&c| c == class)
            && any_or_empty(&f.parties, |m| {
                event.parties.iter().any(|p| {
                    p.login.eq_ignore_ascii_case(&m.login)
                        && (m.roles.is_empty() || m.roles.contains(&p.role))
                })
            })
            && any_or_empty(&f.attributes, |m| {
                event
                    .attributes
                    .get(&m.key)
                    .is_some_and(|v| m.values.is_empty() || m.values.contains(v))
            })
            && self.verdict_passes(event)
    }

    /// Phase 2: an opt-in event passes only when its opt-in is included, a dropped one only
    /// with `include_dropped`; anything else, an unset verdict included, passes.
    fn verdict_passes(&self, event: &WatchEvent) -> bool {
        let Some(forwarding) = &event.forwarding else {
            return true;
        };
        match Verdict::try_from(forwarding.verdict) {
            Ok(Verdict::OptIn) => self.0.include_opt_in.contains(&forwarding.opt_in),
            Ok(Verdict::Drop) => self.0.include_dropped,
            _ => true,
        }
    }
}

/// Whether `values` is empty or one of them matches.
fn any_or_empty<T>(values: &[T], matches: impl FnMut(&T) -> bool) -> bool {
    values.is_empty() || values.iter().any(matches)
}

/// Whether the filter kind `pattern`, exact or a `<domain>.*` prefix, matches `kind`.
fn kind_matches(pattern: &str, kind: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => kind.starts_with(prefix),
        None => pattern == kind,
    }
}

/// Checks every field of `filter`.
fn validate(filter: &WatchFilter) -> Result<(), WatchError> {
    let refuse = |what: String| Err(WatchError::Filter(what));
    if filter.kinds.is_empty() {
        return refuse("`kinds` is empty".to_owned());
    }
    if let Some(kind) = filter.kinds.iter().find(|k| !valid_kind(k)) {
        return refuse(format!(
            "the kind `{kind}` is not `<domain>.<name>` or `<domain>.*`"
        ));
    }
    let repositories = filter.repositories.iter();
    let items = filter.items.iter().map(|i| &i.repository);
    if let Some(repository) = repositories.chain(items).find(|r| !valid_repository(r)) {
        return refuse(format!("the repository `{repository}` is not `owner/name`"));
    }
    if filter.items.iter().any(|i| i.number == 0) {
        return refuse("an item has number zero".to_owned());
    }
    let logins = filter
        .actors
        .iter()
        .chain(filter.parties.iter().map(|p| &p.login));
    if logins
        .chain(filter.parties.iter().flat_map(|p| &p.roles))
        .any(String::is_empty)
    {
        return refuse("an actor login, party login or role is empty".to_owned());
    }
    if let Some(class) = filter
        .actor_classes
        .iter()
        .find(|&&c| ActorClass::try_from(c).is_err() || c == i32::from(ActorClass::Unspecified))
    {
        return refuse(format!("the actor class {class} is unspecified or unknown"));
    }
    if filter.attributes.iter().any(|a| a.key.is_empty()) {
        return refuse("an attribute key is empty".to_owned());
    }
    if filter.include_opt_in.iter().any(String::is_empty) {
        return refuse("an `include_opt_in` entry is empty".to_owned());
    }
    Ok(())
}

/// Whether `kind` is `<domain>.<name>` or `<domain>.*`, each segment `[a-z][a-z0-9_]*`.
fn valid_kind(kind: &str) -> bool {
    kind.split_once('.')
        .is_some_and(|(domain, name)| valid_segment(domain) && (name == "*" || valid_segment(name)))
}

/// Whether `segment` is `[a-z][a-z0-9_]*`.
fn valid_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Whether `repository` is `owner/name`, both parts non-empty.
fn valid_repository(repository: &str) -> bool {
    repository
        .split_once('/')
        .is_some_and(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'))
}
