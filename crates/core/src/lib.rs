//! The mintswarm control plane's library: the store, scheduler, ingest, event log and tracker
//! adapters that the server binary wires together, and the plane's start under its leader lock.

pub mod approvals;
pub mod keeper;
pub mod migrations;
pub mod plane;
pub mod state_sync;
pub mod store;
pub mod testing;
pub mod tracker;
pub mod watch;
