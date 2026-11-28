//! The runner's default activity rules, `defaults/activity/rules.yaml`, embedded at build time.
//!
//! The classifier in `mintswarm_rules::activity` applies them below a project's
//! `metrics.activity.rules`.

use mintswarm_config::{ActivityRule, Error, load_activity_rules};

/// The text of `defaults/activity/rules.yaml`.
pub const DEFAULT_ACTIVITY_RULES: &str = include_str!("../../../defaults/activity/rules.yaml");

/// The default activity rules, parsed from [`DEFAULT_ACTIVITY_RULES`].
pub fn default_activity_rules() -> Result<Vec<ActivityRule>, Error> {
    load_activity_rules(DEFAULT_ACTIVITY_RULES)
}
