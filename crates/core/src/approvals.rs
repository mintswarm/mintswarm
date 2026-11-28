//! Approvals: the home of the plane's approvals evaluator.
//!
//! - [`approver`]: the reads behind who approves, the tracker issue's owner up its parent
//!   chain and the CODEOWNERS approval groups of a pull request's changed paths, fed into the
//!   pure rules of [`mintswarm_rules::approver`].
//! - [`trigger`]: the deliveries that queue an issue for approvals, or a pull request for the
//!   keeper, on the state-sync queue.
//! - [`judge`]: the pure judge of the approve, reject and help comments of an issue.
//! - [`evaluate`]: one approvals evaluation from fresh reads, which the state-sync queue's
//!   worker runs after state-sync; it judges and logs, and writes nothing.

pub mod approver;
pub mod evaluate;
pub mod judge;
pub mod trigger;
