//! The tools behind `mintswarm-runner tool <name>`. The ported dogfood tools keep the command
//! line of the Python script each replaces: [`save_output`], the port of `save-output.py`,
//! [`msw_vcs`], the port of the `msw-vcs` shim, and [`plan_floor`] and [`plan_decision`], the
//! ports of `plan-approval/floor.py` and `decision.py`, which share [`plan_args`].
//! [`pr_labels`] has no Python counterpart: it applies the PR labelling rule to git's history.

pub mod msw_vcs;
pub mod plan_args;
pub mod plan_decision;
pub mod plan_floor;
pub mod pr_labels;
pub mod save_output;
