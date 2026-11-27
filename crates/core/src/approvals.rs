//! Approvals: the home of the plane's approvals evaluator.
//!
//! - [`approver`]: the reads behind who approves, the tracker issue's owner up its parent
//!   chain and the CODEOWNERS approval groups of a pull request's changed paths, fed into the
//!   pure rules of [`mintswarm_rules::approver`].

pub mod approver;
