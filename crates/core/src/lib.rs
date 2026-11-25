//! The mintswarm control plane's library: the store, scheduler, ingest, event log and tracker
//! adapters that the server binary wires together.

#[cfg(feature = "pg-tests")]
pub mod testing;
