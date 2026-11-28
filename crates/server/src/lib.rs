//! The mintswarm control-plane server. [`serve`] runs the control plane under its leader lock
//! with its health routes, and [`webhooks`] is a receiver for the GitHub App's webhooks that
//! appends each verified delivery to a log file, one JSON line per delivery.

pub mod serve;
pub mod webhooks;
