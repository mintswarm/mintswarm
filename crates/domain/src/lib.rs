//! Domain types for mintswarm: the vocabulary shared by the control plane, the runners and the
//! tools.

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
