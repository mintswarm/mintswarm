//! Pure rules shared by the runner and the plane: how a value is written to a tracker field,
//! how a read is interpreted, what counts as drift, and how a size is resolved.
//!
//! Nothing here does I/O or records anything; callers supply what was read and act on the
//! result.

pub mod binding;
pub mod exit;
pub mod field;
pub mod glob;
pub mod json;
pub mod markdown;
pub mod size;
