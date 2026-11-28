//! Support for tests of core and of the crates that build on it: an in-memory tracker, the
//! watch store specification every [`WatchLog`](crate::watch::WatchLog) passes, the store
//! specification every [`Store`](crate::store::Store) passes, and short-lived Postgres instances
//! behind the `pg-tests` feature.

#[cfg(feature = "pg-tests")]
pub mod pg;
pub mod store;
pub mod tracker;
pub mod watch_log;
