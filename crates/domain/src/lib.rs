//! Domain types for mintswarm: the vocabulary shared by the control plane, the runners and the
//! tools.

pub mod cost;
pub mod field;
pub mod ids;
pub mod job;
pub mod plane;
pub mod records;
pub mod scale;
pub mod tracker;
pub mod vcs;
pub mod work;

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
