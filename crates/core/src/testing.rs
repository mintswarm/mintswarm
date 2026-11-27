//! Support for tests of core and of the crates that build on it: an in-memory tracker, the
//! watch store specification every [`WatchLog`](crate::watch::WatchLog) passes, and short-lived
//! Postgres instances behind the `pg-tests` feature.

#[cfg(feature = "pg-tests")]
pub mod pg;
pub mod tracker;
pub mod watch_log;
