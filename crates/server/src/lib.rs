//! The mintswarm control-plane server. [`webhooks`] is a receiver for the GitHub App's webhooks
//! that appends each verified delivery to a log file, one JSON line per delivery.

pub mod webhooks;
