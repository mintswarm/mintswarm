//! Keeping `msw-state` in step with the tracker, design §4–§10.
//!
//! [`trigger`] reduces a normalised delivery to the issues it concerns, [`queue`] holds them in
//! one set per repository with one [`Worker`], [`evaluate`] evaluates one issue from fresh reads
//! under the [`rule`], and [`report`] posts the blocker notes of active issues.

pub mod evaluate;
pub mod queue;
pub mod report;
pub mod rule;
pub mod trigger;

pub use evaluate::{Evaluation, Outcome, Settings};
pub use queue::{StateSync, Worker};
