//! Support for tests of core and of the crates that build on it: an in-memory tracker, and
//! short-lived Postgres instances behind the `pg-tests` feature.

#[cfg(feature = "pg-tests")]
pub mod pg;
pub mod tracker;
