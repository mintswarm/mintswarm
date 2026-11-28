//! The schema of the repository config, `.mintswarm/config.yaml`.
//!
//! The whole document is checked: every key must be known, and the sections the dogfood tools
//! validate are checked as they check them, one check for each of theirs:
//! `plan-approval/floor.py` `validate_config`, `landing/floor.py` `validate_landing`,
//! `dependencies/new-packages.py` `review_mode`, `report-gates.py` `resolve`,
//! `readiness/check.py` `review_config`, `readiness/check.py` `trusted_ingest` for
//! `tracker.ingest.match`, and `github/msw-state.py` `settings`, which covers `tracker.state`,
//! `tracker.plan_verdict`, `tracker.decomp_verdict` and `tracker.short_title`. A change to any of
//! those, or to the config's keys, changes this module in the same change. The
//! `tracker.state.options` check is `state.rs` `check_options`, and the shared cases in
//! `tests/conformance/tracker-state.json` hold it and `settings` to the same outcome and key
//! path.
//!
//! The loader is stricter than the dogfood tools. They read only the keys they use and coerce
//! some mismatched types; this module checks every key and rejects each mismatch instead. A
//! config the tools accept can therefore fail here, for example:
//!
//! - a non-string `agents.stages.<stage>.effort`, which `landing/decision.py` reads as none;
//! - a stage name under `agents.stages` that is not a stage, which the tools never look up;
//! - a non-string `gates.run[].name`, which `plan-approval/floor.py` tolerates;
//! - `gates.report.converters: []`, which `report-gates.py` reads as empty;
//! - a key under `pipeline.dependencies` other than `review`, which `new-packages.py` ignores;
//! - a `field_id` under `tracker.state`, `tracker.plan_verdict`, `tracker.decomp_verdict` or
//!   `tracker.short_title`, which `msw-state.py` ignores: field ids are resolved per account and
//!   are not configuration.
//!
//! It is looser in one place: a missing `tracker.ingest.match` reads as `any`, where
//! `trusted_ingest` reports it as not any or all.
//!
//! The `gates.format` check is the loader's own: no dogfood tool checks it.
//!
//! The `tracker.actions` check is the loader's own as well. When present, the table holds a row
//! for every [`Action`], each with a `people` rule allowed for that row and an `apps` list that
//! is empty in every row but `merge`'s; when absent, no one is trusted with any action.
//!
//! The `tracker.fields` check is the loader's own too. Each `tracker.fields.<role>` entry is
//! checked through the role's [`RoleDescriptor`], found by name in a registry, so the check
//! holds no rule for one role only. A tracker field is named, never given by id: its id is
//! resolved from the name at the tracker edge, so a `field_id` key there is an unknown key.
//!
//! The `metrics` check is the loader's own as well: `metrics` holds only `activity`, which the
//! `activity` module checks with the same schema as the runner's default activity rules,
//! described on [`ActivityRule`].
//!
//! The `principles` check is the loader's own too, in the `principles` module: the packs the
//! repository uses, how each is pinned and its overrides, following `schemas/settings.schema.yaml`
//! in mintswarm/tenets.

use std::collections::BTreeMap;

use mintswarm_domain::field::{
    FieldMap, FieldMapError, FieldMapSpec, FieldSource, LabelPattern, OptionSpec, SourceSpec,
};
use mintswarm_domain::role::{Role, RoleDescriptor, roles};
use mintswarm_domain::tracker::Login;
use serde_json::{Map, Value};

use crate::activity::check_activity;
use crate::principles::{PrincipleSource, check_principles};
use crate::state::check_options;
use crate::{
    Action, ActionTable, ActionTrust, ActivityRule, Error, Path, People, Stage, StateSettings,
    StatusMode, Trust, TrustMatch, parse_yaml,
};

/// The plan-approval and landing modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A human always decides.
    Always,
    /// The checks run and are recorded, while a human decides.
    Shadow,
    /// The checks decide, within their limits.
    Judged,
    /// No approval is asked.
    Never,
}

impl Mode {
    const NAMES: [(&'static str, Mode); 4] = [
        ("always", Mode::Always),
        ("shadow", Mode::Shadow),
        ("judged", Mode::Judged),
        ("never", Mode::Never),
    ];
}

/// `gates.summary`: whether an agent summarises gate failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatesSummary {
    /// An agent summarises them.
    Agent,
    /// They are not summarised; the default.
    Off,
}

/// `pipeline.dependencies.review`: whether new and upgraded packages are judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyReview {
    /// The review stage judges each one.
    Agent,
    /// They are not judged; the default.
    Off,
}

/// `pipeline.review.clear`: the lowest finding severity that must be cleared before a PR is
/// ready, as the dogfood readiness check's `review_config` accepts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReviewClear {
    /// Only blockers.
    Blocker,
    /// Blockers and majors; the default.
    Major,
    /// Everything but nits.
    Minor,
    /// Every finding.
    Nit,
}

impl ReviewClear {
    const NAMES: [(&'static str, ReviewClear); 4] = [
        ("blocker", ReviewClear::Blocker),
        ("major", ReviewClear::Major),
        ("minor", ReviewClear::Minor),
        ("nit", ReviewClear::Nit),
    ];
}

/// The components and paths whose match sends a plan or a change to a human.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequireHuman {
    /// Component names, each defined in `components`.
    pub components: Vec<String>,
    /// Path globs.
    pub paths: Vec<String>,
}

/// `pipeline.plan_approval.limits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Files a plan may touch.
    pub files: u64,
    /// Lines a plan may change.
    pub lines: u64,
    /// New files a plan may add.
    pub new_files: u64,
}

/// One component's entry in `pipeline.plan_approval.limits_by_component`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LimitOverrides {
    /// Overrides `limits.files`.
    pub files: Option<u64>,
    /// Overrides `limits.lines`.
    pub lines: Option<u64>,
    /// Overrides `limits.new_files`.
    pub new_files: Option<u64>,
}

/// `pipeline.plan_approval`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanApproval {
    /// `mode`.
    pub mode: Mode,
    /// `min_confidence`, 90 when absent.
    pub min_confidence: u64,
    /// `require_human`.
    pub require_human: RequireHuman,
    /// `limits`.
    pub limits: Limits,
    /// `limits_by_component`, by component name.
    pub limits_by_component: BTreeMap<String, LimitOverrides>,
    /// `non_production`, the floor's built-in globs when absent.
    pub non_production: Vec<String>,
}

/// `pipeline.landing`.
#[derive(Debug, Clone, PartialEq)]
pub struct Landing {
    /// `mode`.
    pub mode: Mode,
    /// `require_human`.
    pub require_human: RequireHuman,
    /// `footprint.lines_factor`.
    pub lines_factor: f64,
    /// `min_confidence`, 95 when absent.
    pub min_confidence: u64,
}

/// One `gates.format` entry: a formatter and the paths it formats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatter {
    /// `name`, unique among the entries.
    pub name: String,
    /// `command`, a non-empty string that splits as a shell command.
    pub command: String,
    /// `paths`, a non-empty list of path globs.
    pub paths: Vec<String>,
    /// `gate`, the name of the `gates.run` entry that checks the formatting. A `gates.run`
    /// entry without a string `name` cannot be named here.
    pub gate: String,
}

/// A validated repository config.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoConfig {
    value: Value,
    plan_approval: PlanApproval,
    landing: Landing,
    dependency_review: DependencyReview,
    review_max_rounds: Option<u64>,
    review_clear: ReviewClear,
    gates_summary: GatesSummary,
    formatters: Vec<Formatter>,
    trust: Trust,
    state: Option<StateSettings>,
    actions: ActionTable,
    activity_rules: Vec<ActivityRule>,
    principles: Vec<PrincipleSource>,
}

/// The lowest, and default, `pipeline.plan_approval.min_confidence`, and the lowest landing one.
const MIN_CONFIDENCE: u64 = 90;
/// The default `pipeline.landing.min_confidence`.
const LANDING_MIN_CONFIDENCE: u64 = 95;
/// The limit names, in the floor's order.
const LIMITS: [&str; 3] = ["files", "lines", "new_files"];
/// `pipeline.plan_approval.non_production` when absent.
const NON_PRODUCTION: [&str; 9] = [
    "**/tests/**",
    "**/*selftest*",
    "**/test_*.py",
    "**/*_test.*",
    "**/*.test.*",
    "**/*.spec.*",
    "**/fixtures/**",
    "**/Cargo.lock",
    "**/pnpm-lock.yaml",
];
/// The report formats with a built-in converter, besides `ctrf`.
const BUILT_IN_FORMATS: [&str; 1] = ["junit"];

/// Parse and validate a repository config, with the registered roles.
pub fn load_repo_config(text: &str) -> Result<RepoConfig, Error> {
    load_repo_config_with_roles(text, roles())
}

/// Parse and validate a repository config, checking `tracker.fields` against `roles`.
pub fn load_repo_config_with_roles(
    text: &str,
    roles: &[&dyn RoleDescriptor],
) -> Result<RepoConfig, Error> {
    RepoConfig::from_value_with_roles(parse_yaml(text)?, roles)
}

impl RepoConfig {
    /// Validate a config already converted to JSON, with the registered roles.
    pub fn from_value(value: Value) -> Result<Self, Error> {
        Self::from_value_with_roles(value, roles())
    }

    /// Validate a config already converted to JSON, checking `tracker.fields` against `roles`.
    pub fn from_value_with_roles(
        value: Value,
        roles: &[&dyn RoleDescriptor],
    ) -> Result<Self, Error> {
        let root = Path::root();
        let top = mapping(&value, &root)?;
        known(
            top,
            &root,
            &[
                "tracker",
                "design",
                "components",
                "shared",
                "agents",
                "pipeline",
                "gates",
                "metrics",
                "principles",
            ],
        )?;
        let (trust, state, actions) = match top.get("tracker") {
            Some(tracker) => check_tracker(tracker, &root.key("tracker"), roles)?,
            None => (Trust::default(), None, ActionTable::default()),
        };
        if let Some(design) = top.get("design") {
            strings_only(design, &root.key("design"), &["store"], &[])?;
        }
        let components = check_components(top.get("components"), &root.key("components"))?;
        optional_string_list(top.get("shared"), &root.key("shared"))?;
        if let Some(agents) = top.get("agents") {
            check_agents(agents, &root.key("agents"))?;
        }

        let pipeline_path = root.key("pipeline");
        let pipeline = match top.get("pipeline") {
            Some(pipeline @ Value::Object(_)) => mapping(pipeline, &pipeline_path)?,
            _ => {
                return Err(Error::schema(
                    &pipeline_path.key("plan_approval"),
                    "is missing",
                ));
            }
        };
        known(
            pipeline,
            &pipeline_path,
            &["plan_approval", "landing", "dependencies", "review"],
        )?;
        let plan_approval = check_plan_approval(pipeline, &pipeline_path, components)?;
        let landing = check_landing(pipeline, &pipeline_path, components)?;
        let dependency_review = check_dependencies(pipeline, &pipeline_path)?;
        let (review_max_rounds, review_clear) = check_review(pipeline, &pipeline_path)?;
        let Gates {
            summary: gates_summary,
            formatters,
        } = check_gates(top.get("gates"), &root.key("gates"))?;
        let activity_rules = check_metrics(top.get("metrics"), &root.key("metrics"))?;
        let principles = check_principles(top.get("principles"), &root.key("principles"))?;

        Ok(RepoConfig {
            value,
            plan_approval,
            landing,
            dependency_review,
            review_max_rounds,
            review_clear,
            gates_summary,
            formatters,
            trust,
            state,
            actions,
            activity_rules,
            principles,
        })
    }

    /// The validated config, as JSON.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// `pipeline.plan_approval`.
    pub fn plan_approval(&self) -> &PlanApproval {
        &self.plan_approval
    }

    /// `pipeline.landing`.
    pub fn landing(&self) -> &Landing {
        &self.landing
    }

    /// `pipeline.dependencies.review`, `off` when absent.
    pub fn dependency_review(&self) -> DependencyReview {
        self.dependency_review
    }

    /// `pipeline.review.max_rounds`, absent when `pipeline.review` is.
    pub fn review_max_rounds(&self) -> Option<u64> {
        self.review_max_rounds
    }

    /// `pipeline.review.clear`, `major` when absent.
    pub fn review_clear(&self) -> ReviewClear {
        self.review_clear
    }

    /// `gates.summary`, `off` when absent.
    pub fn gates_summary(&self) -> GatesSummary {
        self.gates_summary
    }

    /// `gates.format`, empty when absent, null or empty.
    pub fn formatters(&self) -> &[Formatter] {
        &self.formatters
    }

    /// `tracker.ingest`, trusting nothing when absent.
    pub fn trust(&self) -> &Trust {
        &self.trust
    }

    /// `tracker.state`, `None` when it is absent.
    pub fn state(&self) -> Option<&StateSettings> {
        self.state.as_ref()
    }

    /// `tracker.actions`, trusting no one with any action when absent.
    pub fn actions(&self) -> &ActionTable {
        &self.actions
    }

    /// `metrics.activity.rules`, the project's activity rules in file order, empty when absent.
    pub fn activity_rules(&self) -> &[ActivityRule] {
        &self.activity_rules
    }

    /// The principle packs from `principles.sources`, in order; empty when there are none.
    pub fn principles(&self) -> &[PrincipleSource] {
        &self.principles
    }

    /// The role's map from `tracker.fields.<role>`, `None` when the entry is absent or `off`.
    ///
    /// The entry was checked at load when the role was registered then; an entry that does not
    /// build reads as `None`.
    pub fn field_map<R: Role>(&self) -> Option<FieldMap<R::Scale>> {
        let entry = self.value.get("tracker")?.get("fields")?.get(R::NAME)?;
        let path = Path::root().key("tracker").key("fields").key(R::NAME);
        let spec = field_spec(entry, &path).ok()??;
        FieldMap::new(spec).ok()
    }
}

pub(crate) fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a bool",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "a mapping",
    }
}

pub(crate) fn mapping<'a>(value: &'a Value, path: &Path) -> Result<&'a Map<String, Value>, Error> {
    value
        .as_object()
        .ok_or_else(|| Error::schema(path, format!("must be a mapping, not {}", kind(value))))
}

/// Rejects the first key of `map` that is not in `allowed`, naming it.
pub(crate) fn known(map: &Map<String, Value>, path: &Path, allowed: &[&str]) -> Result<(), Error> {
    match map.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(Error::schema(&path.key(key), "unknown key")),
        None => Ok(()),
    }
}

pub(crate) fn string<'a>(value: &'a Value, path: &Path) -> Result<&'a str, Error> {
    value
        .as_str()
        .ok_or_else(|| Error::schema(path, format!("must be a string, not {}", kind(value))))
}

pub(crate) fn string_list(value: &Value, path: &Path) -> Result<Vec<String>, Error> {
    let error = || Error::schema(path, "must be a list of strings");
    let items = value.as_array().ok_or_else(error)?;
    items
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or_else(error))
        .collect()
}

/// A string list where absent or null reads as empty, as the floor's `string_list` reads it.
fn optional_string_list(value: Option<&Value>, path: &Path) -> Result<Vec<String>, Error> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => string_list(value, path),
    }
}

/// A mapping with only the keys `scalars` (strings) and `lists` (string lists).
fn strings_only(value: &Value, path: &Path, scalars: &[&str], lists: &[&str]) -> Result<(), Error> {
    let map = mapping(value, path)?;
    let allowed: Vec<&str> = scalars.iter().chain(lists).copied().collect();
    known(map, path, &allowed)?;
    for (key, value) in map {
        if lists.contains(&key.as_str()) {
            string_list(value, &path.key(key))?;
        } else {
            string(value, &path.key(key))?;
        }
    }
    Ok(())
}

/// An integer, not a bool, as the floors' `isinstance(v, int) and not isinstance(v, bool)`.
fn integer(value: &Value) -> Option<i128> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(i128::from)),
        _ => None,
    }
}

/// An integer of at least `low`, and at most `high` when given.
fn bounded(value: &Value, path: &Path, low: u64, high: Option<u64>) -> Result<u64, Error> {
    let message = match high {
        Some(high) => format!("must be an integer from {low} to {high}"),
        None if low == 0 => "must be a non-negative integer".to_owned(),
        None => format!("must be an integer of {low} or more"),
    };
    integer(value)
        .filter(|&n| n >= i128::from(low) && high.is_none_or(|high| n <= i128::from(high)))
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| Error::schema(path, format!("{message}, not {value}")))
}

/// `metrics`: only `activity`, checked as the activity rules are; no rules when either is absent.
fn check_metrics(value: Option<&Value>, path: &Path) -> Result<Vec<ActivityRule>, Error> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let map = mapping(value, path)?;
    known(map, path, &["activity"])?;
    match map.get("activity") {
        Some(activity) => check_activity(activity, &path.key("activity")),
        None => Ok(Vec::new()),
    }
}

fn check_tracker(
    value: &Value,
    path: &Path,
    roles: &[&dyn RoleDescriptor],
) -> Result<(Trust, Option<StateSettings>, ActionTable), Error> {
    let tracker = mapping(value, path)?;
    known(
        tracker,
        path,
        &[
            "kind",
            "kinds",
            "components",
            "state",
            "plan_verdict",
            "decomp_verdict",
            "short_title",
            "fields",
            "ingest",
            "actions",
        ],
    )?;
    if let Some(kind) = tracker.get("kind") {
        string(kind, &path.key("kind"))?;
    }
    if let Some(kinds) = tracker.get("kinds") {
        let kinds_path = path.key("kinds");
        let map = mapping(kinds, &kinds_path)?;
        known(
            map,
            &kinds_path,
            &["source", "design", "plan", "implementation"],
        )?;
        for (key, value) in map {
            if key == "source" {
                string(value, &kinds_path.key(key))?;
            } else {
                strings_only(value, &kinds_path.key(key), &["issue_type", "label"], &[])?;
            }
        }
    }
    if let Some(components) = tracker.get("components") {
        strings_only(
            components,
            &path.key("components"),
            &["source", "label", "field"],
            &[],
        )?;
    }
    let state = match tracker.get("state") {
        Some(state) => check_tracker_value(state, &path.key("state"), true)?,
        None => None,
    };
    for key in ["plan_verdict", "decomp_verdict", "short_title"] {
        if let Some(verdict) = tracker.get(key) {
            check_tracker_value(verdict, &path.key(key), false)?;
        }
    }
    if let Some(fields) = tracker.get("fields") {
        check_fields(fields, &path.key("fields"), roles)?;
    }
    let trust = match tracker.get("ingest") {
        Some(ingest) => check_ingest(ingest, &path.key("ingest"))?,
        None => Trust::default(),
    };
    let actions = match tracker.get("actions") {
        Some(actions) => check_actions(actions, &path.key("actions"))?,
        None => ActionTable::default(),
    };
    Ok((trust, state, actions))
}

/// `tracker.ingest`, built into a [`Trust`]. A missing `match` reads as `any`.
fn check_ingest(value: &Value, path: &Path) -> Result<Trust, Error> {
    let map = mapping(value, path)?;
    known(map, path, &["trusted", "match"])?;
    let mut trust = Trust::default();
    if let Some(trusted) = map.get("trusted") {
        let trusted_path = path.key("trusted");
        let trusted = mapping(trusted, &trusted_path)?;
        known(trusted, &trusted_path, &["labels", "authors"])?;
        trust.labels = optional_string_list(trusted.get("labels"), &trusted_path.key("labels"))?;
        trust.authors = optional_string_list(trusted.get("authors"), &trusted_path.key("authors"))?
            .into_iter()
            .map(Login)
            .collect();
    }
    if let Some(value) = map.get("match") {
        let found = value
            .as_str()
            .and_then(|name| TrustMatch::NAMES.iter().find(|(known, _)| *known == name));
        trust.r#match = match found {
            Some((_, found)) => *found,
            None => {
                return Err(Error::schema(
                    &path.key("match"),
                    format!("must be any or all, not {value}"),
                ));
            }
        };
    }
    Ok(trust)
}

/// `tracker.actions`, built into an [`ActionTable`]: every action has a row, and no other key is
/// allowed.
fn check_actions(value: &Value, path: &Path) -> Result<ActionTable, Error> {
    let map = mapping(value, path)?;
    let names = Action::NAMES.map(|(name, _)| name);
    known(map, path, &names)?;
    let mut table = ActionTable::default();
    for action in Action::ALL {
        let row_path = path.key(action.as_str());
        let row = map
            .get(action.as_str())
            .ok_or_else(|| Error::schema(&row_path, "is missing"))?;
        *table.get_mut(action) = check_action_row(action, row, &row_path)?;
    }
    Ok(table)
}

/// One `tracker.actions.<action>` row: `people` and `apps`, both required.
fn check_action_row(action: Action, value: &Value, path: &Path) -> Result<ActionTrust, Error> {
    let row = mapping(value, path)?;
    known(row, path, &["people", "apps"])?;
    let required = |key: &str| {
        row.get(key)
            .ok_or_else(|| Error::schema(&path.key(key), "is missing"))
    };

    let people_path = path.key("people");
    let value = required("people")?;
    let people = value
        .as_str()
        .and_then(|name| People::NAMES.iter().find(|(known, _)| *known == name))
        .map(|(_, people)| *people)
        .ok_or_else(|| {
            let names = People::NAMES.map(|(name, _)| name).join(", ");
            Error::schema(&people_path, format!("must be one of {names}, not {value}"))
        })?;
    if !people.allowed_in(action) {
        return Err(Error::schema(
            &people_path,
            format!("{value} is not allowed for {}", action.as_str()),
        ));
    }

    let apps_path = path.key("apps");
    let apps = string_list(required("apps")?, &apps_path)?;
    for (index, app) in apps.iter().enumerate() {
        if app.ends_with("[bot]") {
            return Err(Error::schema(
                &apps_path,
                format!("{app:?} must be an App slug, without [bot]"),
            ));
        }
        if !is_app_slug(app) {
            return Err(Error::schema(
                &apps_path,
                format!("{app:?} must be an App slug: lowercase letters, digits and hyphens"),
            ));
        }
        if apps[..index].contains(app) {
            return Err(Error::schema(
                &apps_path,
                format!("{app:?} is listed twice"),
            ));
        }
    }
    if action != Action::Merge && !apps.is_empty() {
        return Err(Error::schema(
            &apps_path,
            format!(
                "must be empty: only {} may trust an App",
                Action::Merge.as_str()
            ),
        ));
    }
    Ok(ActionTrust { people, apps })
}

/// Whether `app` is a GitHub App slug: non-empty, lowercase ASCII letters, digits and hyphens.
fn is_app_slug(app: &str) -> bool {
    !app.is_empty()
        && app
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// `tracker.state` (`with_status`), `tracker.plan_verdict`, `tracker.decomp_verdict` or
/// `tracker.short_title`, checked as `msw-state.py` `settings` checks it. Fields are named only:
/// their ids are resolved per account, so a `field_id` key is unknown here.
///
/// `tracker.state` also takes `options`, and is built into [`StateSettings`]; the other blocks
/// give `None`.
fn check_tracker_value(
    value: &Value,
    path: &Path,
    with_status: bool,
) -> Result<Option<StateSettings>, Error> {
    let block = mapping(value, path)?;
    let keys: &[&str] = if with_status {
        &["source", "field", "label", "status", "options"]
    } else {
        &["source", "field", "label"]
    };
    known(block, path, keys)?;
    let one_of = |key: &str, allowed: [&str; 2]| match block.get(key) {
        Some(Value::String(value)) if allowed.contains(&value.as_str()) => Ok(value.as_str()),
        other => Err(Error::schema(
            &path.key(key),
            format!(
                "must be {} or {}, not {}",
                allowed[0],
                allowed[1],
                other.unwrap_or(&Value::Null)
            ),
        )),
    };
    let field = if one_of("source", ["field", "labels"])? == "field" {
        match block.get("field") {
            Some(Value::String(field)) if !field.trim().is_empty() => Some(field),
            _ => return Err(Error::schema(&path.key("field"), "must be a field name")),
        }
    } else {
        None
    };
    let label = match block.get("label") {
        Some(Value::String(label)) if LabelPattern::new(label).is_some() => label,
        _ => {
            return Err(Error::schema(
                &path.key("label"),
                format!("must hold {OPTION} exactly once"),
            ));
        }
    };
    if !with_status {
        return Ok(None);
    }
    let status = match one_of("status", ["issue_status", "off"])? {
        "issue_status" => StatusMode::IssueStatus,
        _ => StatusMode::Off,
    };
    let options = check_options(block.get("options"), &path.key("options"))?;
    let source = match field {
        Some(name) => FieldSource::Field { name: name.clone() },
        None => FieldSource::Labels {
            label: label.clone(),
        },
    };
    Ok(Some(StateSettings {
        source,
        status,
        options,
    }))
}

/// `tracker.fields`: each `<role>` entry is checked through the registered descriptor of that
/// name, so the check holds no rule for one role only.
fn check_fields(value: &Value, path: &Path, roles: &[&dyn RoleDescriptor]) -> Result<(), Error> {
    for (name, entry) in mapping(value, path)? {
        let role_path = path.key(name);
        let Some(role) = roles.iter().find(|role| role.name() == name) else {
            let known: Vec<&str> = roles.iter().map(|role| role.name()).collect();
            return Err(Error::schema(
                &role_path,
                format!(
                    "is not a registered role; the roles are {}",
                    known.join(", ")
                ),
            ));
        };
        if let Some(spec) = field_spec(entry, &role_path)? {
            role.check(spec)
                .map_err(|error| field_map_error(&error, &role_path))?;
        }
    }
    Ok(())
}

/// One `tracker.fields.<role>` entry, `None` for `source: off`, whose keys are still checked.
fn field_spec(value: &Value, path: &Path) -> Result<Option<FieldMapSpec>, Error> {
    let entry = mapping(value, path)?;
    known(
        entry,
        path,
        &["source", "field", "label", "default", "options"],
    )?;
    let source_path = path.key("source");
    let source = match entry.get("source") {
        Some(Value::String(source)) if ["field", "labels", "off"].contains(&source.as_str()) => {
            source.as_str()
        }
        other => {
            return Err(Error::schema(
                &source_path,
                format!(
                    "must be field, labels or off, not {}",
                    other.unwrap_or(&Value::Null)
                ),
            ));
        }
    };
    let field_path = path.key("field");
    let field = entry
        .get("field")
        .map(|field| string(field, &field_path))
        .transpose()?;
    let label_path = path.key("label");
    let label = match entry.get("label") {
        None => None,
        Some(label) => match string(label, &label_path)? {
            label if LabelPattern::new(label).is_some() => Some(label),
            _ => {
                return Err(Error::schema(
                    &label_path,
                    format!("must hold {OPTION} exactly once"),
                ));
            }
        },
    };
    let default_path = path.key("default");
    let default = entry
        .get("default")
        .map(|default| string(default, &default_path))
        .transpose()?;
    let options_path = path.key("options");
    let options = entry
        .get("options")
        .map(|options| option_specs(options, &options_path))
        .transpose()?;

    let source = match source {
        "off" => return Ok(None),
        "field" => match field {
            Some(name) if !name.trim().is_empty() => SourceSpec::Field {
                name: name.to_owned(),
            },
            _ => return Err(Error::schema(&field_path, "must be a field name")),
        },
        _ => match label {
            Some(label) => SourceSpec::Labels {
                label: label.to_owned(),
            },
            None => return Err(Error::schema(&label_path, "is missing")),
        },
    };
    let default = default.ok_or_else(|| Error::schema(&default_path, "is missing"))?;
    let options = options.ok_or_else(|| Error::schema(&options_path, "is missing"))?;
    Ok(Some(FieldMapSpec {
        source,
        default: default.to_owned(),
        options,
    }))
}

/// The placeholder a field label holds once.
const OPTION: &str = mintswarm_domain::field::OPTION;

/// `tracker.fields.<role>.options`: option names mapped to `range`, `write` and `reads_as`.
fn option_specs(value: &Value, path: &Path) -> Result<Vec<OptionSpec>, Error> {
    let mut options = Vec::new();
    for (name, option) in mapping(value, path)? {
        let option_path = path.key(name);
        let map = mapping(option, &option_path)?;
        known(map, &option_path, &["range", "write", "reads_as"])?;
        let range_path = option_path.key("range");
        let range = match map.get("range") {
            Some(range) => string(range, &range_path)?.to_owned(),
            None => return Err(Error::schema(&range_path, "is missing")),
        };
        let write = match map.get("write") {
            None => true,
            Some(Value::Bool(write)) => *write,
            Some(other) => {
                return Err(Error::schema(
                    &option_path.key("write"),
                    format!("must be a bool, not {}", kind(other)),
                ));
            }
        };
        let reads_as = map
            .get("reads_as")
            .map(|reads_as| string(reads_as, &option_path.key("reads_as")).map(str::to_owned))
            .transpose()?;
        options.push(OptionSpec {
            name: name.clone(),
            range,
            write,
            reads_as,
        });
    }
    Ok(options)
}

/// A broken invariant as a schema error at the key it concerns under `role`.
fn field_map_error(error: &FieldMapError, role: &Path) -> Error {
    let options = role.key("options");
    let path = match error {
        FieldMapError::UnknownDefault { .. } => role.key("default"),
        FieldMapError::LabelPattern { .. } => role.key("label"),
        FieldMapError::NoOptions | FieldMapError::Uncovered { .. } => options,
        FieldMapError::DuplicateOption { option }
        | FieldMapError::BadRange { option, .. }
        | FieldMapError::UnknownReadsAs { option, .. }
        | FieldMapError::ReadsAsOutsideRange { option, .. } => options.key(option),
        FieldMapError::Overlap { second, .. } => options.key(second),
    };
    Error::schema(&path, error.to_string())
}

/// The component names, after checking each component as `validate_config` does.
fn check_components<'a>(
    value: Option<&'a Value>,
    path: &Path,
) -> Result<&'a Map<String, Value>, Error> {
    let components = match value {
        Some(value) => mapping(value, path)?,
        None => return Err(Error::schema(path, "is missing")),
    };
    for (name, spec) in components {
        let spec_path = path.key(name);
        let map = mapping(spec, &spec_path)?;
        known(map, &spec_path, &["description", "paths", "reviewers"])?;
        if let Some(description) = map.get("description") {
            string(description, &spec_path.key("description"))?;
        }
        optional_string_list(map.get("paths"), &spec_path.key("paths"))?;
        if let Some(reviewers) = map.get("reviewers") {
            string_list(reviewers, &spec_path.key("reviewers"))?;
        }
    }
    Ok(components)
}

fn check_agents(value: &Value, path: &Path) -> Result<(), Error> {
    let agents = mapping(value, path)?;
    known(agents, path, &["default", "stages"])?;
    let choice = ["harness", "model", "effort"];
    if let Some(default) = agents.get("default") {
        strings_only(default, &path.key("default"), &choice, &[])?;
    }
    if let Some(stages) = agents.get("stages") {
        let stages_path = path.key("stages");
        for (stage, value) in mapping(stages, &stages_path)? {
            let stage_path = stages_path.key(stage);
            if Stage::from_name(stage).is_none() {
                return Err(Error::schema(&stage_path, "unknown key"));
            }
            strings_only(value, &stage_path, &choice, &[])?;
        }
    }
    Ok(())
}

fn mode(value: Option<&Value>, path: &Path) -> Result<Mode, Error> {
    let found = value
        .and_then(Value::as_str)
        .and_then(|name| Mode::NAMES.iter().find(|(known, _)| *known == name));
    match found {
        Some((_, mode)) => Ok(*mode),
        None => Err(Error::schema(
            path,
            format!(
                "must be one of always, shadow, judged, never, not {}",
                value.map_or_else(|| "absent".to_owned(), Value::to_string)
            ),
        )),
    }
}

/// `require_human`, absent or null reading as empty, with its components defined.
fn require_human(
    value: Option<&Value>,
    path: &Path,
    components: &Map<String, Value>,
) -> Result<RequireHuman, Error> {
    let map = match value {
        None | Some(Value::Null) => return Ok(RequireHuman::default()),
        Some(value) => mapping(value, path)?,
    };
    known(map, path, &["components", "paths"])?;
    let names = optional_string_list(map.get("components"), &path.key("components"))?;
    let paths = optional_string_list(map.get("paths"), &path.key("paths"))?;
    if let Some(undefined) = names.iter().find(|name| !components.contains_key(*name)) {
        return Err(Error::schema(
            &path.key("components"),
            format!("names an undefined component: {undefined}"),
        ));
    }
    Ok(RequireHuman {
        components: names,
        paths,
    })
}

fn check_plan_approval(
    pipeline: &Map<String, Value>,
    pipeline_path: &Path,
    components: &Map<String, Value>,
) -> Result<PlanApproval, Error> {
    let path = pipeline_path.key("plan_approval");
    let approval = match pipeline.get("plan_approval") {
        Some(value) => mapping(value, &path)?,
        None => return Err(Error::schema(&path, "is missing")),
    };
    known(
        approval,
        &path,
        &[
            "mode",
            "require_human",
            "limits",
            "min_confidence",
            "non_production",
            "limits_by_component",
        ],
    )?;
    let mode = mode(approval.get("mode"), &path.key("mode"))?;
    let min_confidence = match approval.get("min_confidence") {
        None => MIN_CONFIDENCE,
        Some(value) => bounded(
            value,
            &path.key("min_confidence"),
            MIN_CONFIDENCE,
            Some(100),
        )?,
    };
    let require_human = require_human(
        approval.get("require_human"),
        &path.key("require_human"),
        components,
    )?;

    let limits_path = path.key("limits");
    let limits = match approval.get("limits") {
        Some(Value::Object(limits)) => limits,
        _ => return Err(Error::schema(&limits_path, "is missing or not a mapping")),
    };
    known(limits, &limits_path, &LIMITS)?;
    let mut values = [0; 3];
    for (slot, key) in values.iter_mut().zip(LIMITS) {
        *slot = match limits.get(key) {
            Some(value) => bounded(value, &limits_path.key(key), 0, None)?,
            None => return Err(Error::schema(&limits_path.key(key), "is missing")),
        };
    }
    let [files, lines, new_files] = values;
    let limits = Limits {
        files,
        lines,
        new_files,
    };

    let non_production = match approval.get("non_production") {
        None => NON_PRODUCTION.iter().map(|&glob| glob.to_owned()).collect(),
        Some(value) => string_list(value, &path.key("non_production"))?,
    };

    let by_path = path.key("limits_by_component");
    let mut limits_by_component = BTreeMap::new();
    if let Some(value) = approval.get("limits_by_component") {
        for (name, overrides) in mapping(value, &by_path)? {
            let where_ = by_path.key(name);
            if !components.contains_key(name) {
                return Err(Error::schema(&where_, "names an undefined component"));
            }
            let map = mapping(overrides, &where_)?;
            known(map, &where_, &LIMITS)?;
            let get = |key: &str| {
                map.get(key)
                    .map(|value| bounded(value, &where_.key(key), 0, None))
                    .transpose()
            };
            let entry = LimitOverrides {
                files: get("files")?,
                lines: get("lines")?,
                new_files: get("new_files")?,
            };
            limits_by_component.insert(name.clone(), entry);
        }
    }

    Ok(PlanApproval {
        mode,
        min_confidence,
        require_human,
        limits,
        limits_by_component,
        non_production,
    })
}

fn check_landing(
    pipeline: &Map<String, Value>,
    pipeline_path: &Path,
    components: &Map<String, Value>,
) -> Result<Landing, Error> {
    let path = pipeline_path.key("landing");
    let landing = match pipeline.get("landing") {
        Some(value) => mapping(value, &path)?,
        None => return Err(Error::schema(&path, "is missing")),
    };
    known(
        landing,
        &path,
        &["mode", "require_human", "footprint", "min_confidence"],
    )?;
    let mode = mode(landing.get("mode"), &path.key("mode"))?;
    let require_human = require_human(
        landing.get("require_human"),
        &path.key("require_human"),
        components,
    )?;

    let footprint_path = path.key("footprint");
    let footprint = match landing.get("footprint") {
        Some(Value::Object(footprint)) => footprint,
        _ => {
            return Err(Error::schema(
                &footprint_path,
                "is missing or not a mapping",
            ));
        }
    };
    known(footprint, &footprint_path, &["lines_factor"])?;
    let factor_path = footprint_path.key("lines_factor");
    let lines_factor = match footprint.get("lines_factor") {
        Some(Value::Number(number)) => number.as_f64().filter(|f| f.is_finite() && *f >= 1.0),
        Some(_) => None,
        None => return Err(Error::schema(&factor_path, "is missing")),
    }
    .ok_or_else(|| Error::schema(&factor_path, "must be a number of 1 or more"))?;

    let min_confidence = match landing.get("min_confidence") {
        None => LANDING_MIN_CONFIDENCE,
        Some(value) => bounded(
            value,
            &path.key("min_confidence"),
            MIN_CONFIDENCE,
            Some(100),
        )?,
    };
    Ok(Landing {
        mode,
        require_human,
        lines_factor,
        min_confidence,
    })
}

fn check_dependencies(
    pipeline: &Map<String, Value>,
    pipeline_path: &Path,
) -> Result<DependencyReview, Error> {
    let path = pipeline_path.key("dependencies");
    let dependencies = match pipeline.get("dependencies") {
        None | Some(Value::Null) => return Ok(DependencyReview::Off),
        Some(value) => mapping(value, &path)?,
    };
    known(dependencies, &path, &["review"])?;
    match dependencies.get("review") {
        None | Some(Value::Null) => Ok(DependencyReview::Off),
        Some(Value::String(value)) if value == "off" => Ok(DependencyReview::Off),
        Some(Value::String(value)) if value == "agent" => Ok(DependencyReview::Agent),
        Some(value) => Err(Error::schema(
            &path.key("review"),
            format!("must be agent or off, not {value}"),
        )),
    }
}

fn check_review(
    pipeline: &Map<String, Value>,
    pipeline_path: &Path,
) -> Result<(Option<u64>, ReviewClear), Error> {
    let path = pipeline_path.key("review");
    let Some(value) = pipeline.get("review") else {
        return Ok((None, ReviewClear::Major));
    };
    let review = mapping(value, &path)?;
    known(review, &path, &["max_rounds", "reviewers", "clear"])?;
    if let Some(reviewers) = review.get("reviewers") {
        string_list(reviewers, &path.key("reviewers"))?;
    }
    let clear = match review.get("clear") {
        None | Some(Value::Null) => ReviewClear::Major,
        Some(value) => value
            .as_str()
            .and_then(|name| {
                ReviewClear::NAMES
                    .iter()
                    .find(|(known, _)| *known == name)
                    .map(|(_, clear)| *clear)
            })
            .ok_or_else(|| {
                Error::schema(
                    &path.key("clear"),
                    format!("must be blocker, major, minor or nit, not {value}"),
                )
            })?,
    };
    let max_rounds = match review.get("max_rounds") {
        Some(value) => bounded(value, &path.key("max_rounds"), 1, None)?,
        None => return Err(Error::schema(&path.key("max_rounds"), "is missing")),
    };
    Ok((Some(max_rounds), clear))
}

/// What `check_gates` reads from `gates`.
struct Gates {
    summary: GatesSummary,
    formatters: Vec<Formatter>,
}

fn check_gates(value: Option<&Value>, path: &Path) -> Result<Gates, Error> {
    let gates = match value {
        None | Some(Value::Null) => {
            return Ok(Gates {
                summary: GatesSummary::Off,
                formatters: Vec::new(),
            });
        }
        Some(value) => mapping(value, path)?,
    };
    known(
        gates,
        path,
        &["tests", "run", "summary", "report", "format"],
    )?;
    match gates.get("tests") {
        None | Some(Value::Null) => {}
        Some(value) => {
            string(value, &path.key("tests"))?;
        }
    }
    let run_path = path.key("run");
    let run = match gates.get("run") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(run)) => run.as_slice(),
        Some(value) => {
            return Err(Error::schema(
                &run_path,
                format!("must be a list, not {}", kind(value)),
            ));
        }
    };
    for (index, gate) in run.iter().enumerate() {
        check_gate(gate, &run_path.index(index))?;
    }
    if let Some(report) = gates.get("report") {
        check_report(report, &path.key("report"))?;
    }
    let formatters = check_formatters(gates.get("format"), &path.key("format"), run)?;
    let summary = match gates.get("summary") {
        None | Some(Value::Null) => GatesSummary::Off,
        Some(Value::String(value)) if value == "off" => GatesSummary::Off,
        Some(Value::String(value)) if value == "agent" => GatesSummary::Agent,
        Some(value) => {
            return Err(Error::schema(
                &path.key("summary"),
                format!("must be agent or off, not {value}"),
            ));
        }
    };
    Ok(Gates {
        summary,
        formatters,
    })
}

/// `gates.format`, absent or null reading as empty, each entry naming a `gates.run` entry.
fn check_formatters(
    value: Option<&Value>,
    path: &Path,
    run: &[Value],
) -> Result<Vec<Formatter>, Error> {
    let entries = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(entries)) => entries,
        Some(value) => {
            return Err(Error::schema(
                path,
                format!("must be a list, not {}", kind(value)),
            ));
        }
    };
    let gates: Vec<&str> = run
        .iter()
        .filter_map(|gate| gate.get("name").and_then(Value::as_str))
        .collect();
    let mut formatters: Vec<Formatter> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let formatter = check_formatter(entry, path, index, &formatters, &gates)?;
        formatters.push(formatter);
    }
    Ok(formatters)
}

fn check_formatter(
    entry: &Value,
    list: &Path,
    index: usize,
    earlier: &[Formatter],
    gates: &[&str],
) -> Result<Formatter, Error> {
    let path = &list.index(index);
    let map = mapping(entry, path)?;
    known(map, path, &["name", "command", "paths", "gate"])?;
    let required = |key: &str| {
        map.get(key)
            .ok_or_else(|| Error::schema(&path.key(key), "is missing"))
    };

    let name_path = path.key("name");
    let name = match required("name")? {
        Value::String(name) if !name.trim().is_empty() => name.clone(),
        _ => return Err(Error::schema(&name_path, "must be a non-empty string")),
    };
    if let Some(other) = earlier.iter().position(|f| f.name == name) {
        return Err(Error::schema(
            &name_path,
            format!("{name} is already the name of {}", list.index(other)),
        ));
    }

    let command_path = path.key("command");
    let command = match required("command")? {
        Value::String(command) if !command.trim().is_empty() => command.clone(),
        _ => return Err(Error::schema(&command_path, "must be a non-empty string")),
    };
    shell_split_ok(&command)
        .map_err(|problem| Error::schema(&command_path, format!("does not split: {problem}")))?;

    let paths_path = path.key("paths");
    let paths_error =
        || Error::schema(&paths_path, "must be a non-empty list of non-empty strings");
    let paths = map
        .get("paths")
        .and_then(Value::as_array)
        .filter(|paths| !paths.is_empty())
        .ok_or_else(paths_error)?
        .iter()
        .map(|glob| match glob {
            Value::String(glob) if !glob.is_empty() => Ok(glob.clone()),
            _ => Err(paths_error()),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let gate_path = path.key("gate");
    let gate = string(required("gate")?, &gate_path)?.to_owned();
    if !gates.contains(&gate.as_str()) {
        return Err(Error::schema(
            &gate_path,
            format!("formatter {name} names gate {gate}, which is no gates.run entry"),
        ));
    }

    Ok(Formatter {
        name,
        command,
        paths,
        gate,
    })
}

fn check_gate(gate: &Value, path: &Path) -> Result<(), Error> {
    let map = gate
        .as_object()
        .ok_or_else(|| Error::schema(path, "must be a mapping with a command string"))?;
    known(map, path, &["name", "command", "timeout"])?;
    if let Some(name) = map.get("name") {
        string(name, &path.key("name"))?;
    }
    let command_path = path.key("command");
    let command = match map.get("command") {
        Some(Value::String(command)) => command,
        _ => return Err(Error::schema(&command_path, "must be a string")),
    };
    shell_split_ok(command)
        .map_err(|problem| Error::schema(&command_path, format!("does not split: {problem}")))?;
    if let Some(timeout) = map.get("timeout") {
        bounded(timeout, &path.key("timeout"), 1, None)?;
    }
    Ok(())
}

/// Whether POSIX `shlex.split` accepts `command`: every quote is closed and no escape is left
/// without the character it escapes.
fn shell_split_ok(command: &str) -> Result<(), &'static str> {
    enum State {
        Plain,
        Single,
        Double,
    }
    const UNCLOSED: &str = "No closing quotation";
    const ESCAPE: &str = "No escaped character";
    let mut state = State::Plain;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (&state, c) {
            (State::Plain, '\\') | (State::Double, '\\') => {
                if chars.next().is_none() {
                    return Err(ESCAPE);
                }
            }
            (State::Plain, '\'') => state = State::Single,
            (State::Plain, '"') => state = State::Double,
            (State::Single, '\'') | (State::Double, '"') => state = State::Plain,
            _ => {}
        }
    }
    match state {
        State::Plain => Ok(()),
        State::Single | State::Double => Err(UNCLOSED),
    }
}

/// A format name: `[a-z0-9][a-z0-9-]*`.
fn format_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn check_report(value: &Value, path: &Path) -> Result<(), Error> {
    let report = match value {
        Value::Null => return Ok(()),
        value => mapping(value, path)?,
    };
    known(report, path, &["formats", "converters"])?;
    let converters_path = path.key("converters");
    let mut declared: Vec<&str> = Vec::new();
    match report.get("converters") {
        None | Some(Value::Null) => {}
        Some(value) => {
            for (name, converter) in mapping(value, &converters_path)? {
                check_converter(name, converter, &converters_path.key(name))?;
                declared.push(name);
            }
        }
    }
    let formats_path = path.key("formats");
    let formats = match report.get("formats") {
        None => vec!["ctrf".to_owned()],
        Some(value) => string_list(value, &formats_path)?,
    };
    let mut seen: Vec<&str> = Vec::new();
    for name in &formats {
        if !format_name(name) {
            return Err(Error::schema(
                &formats_path,
                format!("{name} must match [a-z0-9][a-z0-9-]*"),
            ));
        }
        if seen.contains(&name.as_str()) {
            return Err(Error::schema(
                &formats_path,
                format!("{name} is listed twice"),
            ));
        }
        seen.push(name);
        if name != "ctrf"
            && !BUILT_IN_FORMATS.contains(&name.as_str())
            && !declared.contains(&name.as_str())
        {
            return Err(Error::schema(
                &formats_path,
                format!("no converter for {name}"),
            ));
        }
    }
    Ok(())
}

fn check_converter(name: &str, value: &Value, path: &Path) -> Result<(), Error> {
    if !format_name(name) {
        return Err(Error::schema(
            path,
            "a format name must match [a-z0-9][a-z0-9-]*",
        ));
    }
    if name == "ctrf" {
        return Err(Error::schema(
            path,
            "ctrf is reserved and cannot be a converter",
        ));
    }
    let map = mapping(value, path)?;
    known(map, path, &["command", "extension", "timeout"])?;
    let command_path = path.key("command");
    match map.get("command") {
        Some(command) if !string_list(command, &command_path)?.is_empty() => {}
        _ => {
            return Err(Error::schema(
                &command_path,
                "must be a non-empty list of strings",
            ));
        }
    }
    let extension = map.get("extension").and_then(Value::as_str);
    if !extension.is_some_and(|e| {
        !e.is_empty()
            && e.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    }) {
        return Err(Error::schema(
            &path.key("extension"),
            "must match [a-z0-9]+",
        ));
    }
    if let Some(timeout) = map.get("timeout") {
        bounded(timeout, &path.key("timeout"), 1, None)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{format_name, shell_split_ok};

    #[test]
    fn shell_split_finds_unclosed_quotes_and_escapes() {
        assert!(shell_split_ok("cd ./web && bash x.sh").is_ok());
        assert!(shell_split_ok(r#"echo "a \" b" 'c\'"#).is_ok());
        assert!(shell_split_ok("echo 'a").is_err());
        assert!(shell_split_ok("echo \"a").is_err());
        assert!(shell_split_ok("echo a\\").is_err());
        assert!(shell_split_ok("echo \"a\\").is_err());
    }

    #[test]
    fn format_names() {
        assert!(format_name("junit") && format_name("0-x"));
        assert!(!format_name("") && !format_name("-x") && !format_name("Junit"));
    }
}
