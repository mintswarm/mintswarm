//! Domain types for mintswarm: the vocabulary shared by the control plane, the runners and the
//! tools.

pub mod codeowners;
pub mod cost;
pub mod field;
pub mod hosting;
pub mod ids;
pub mod job;
pub mod plane;
pub mod priority;
pub mod probe;
pub mod question;
pub mod records;
pub mod retry;
pub mod role;
pub mod scale;
pub mod time;
pub mod tracker;
pub mod vcs;
pub mod work;

pub use question::{
    Question, QuestionKind, QuestionOption, QuestionShape, QuestionViolation, Recommendation,
    Reference, ReferenceKind,
};

/// The name of this crate, as Cargo knows it.
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::CRATE_NAME;

    #[test]
    fn crate_name_is_mintswarm_domain() {
        assert_eq!(CRATE_NAME, "mintswarm-domain");
    }
}
