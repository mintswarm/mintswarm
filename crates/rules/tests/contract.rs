//! The exit-code contract and the sha256 bindings.

use mintswarm_rules::binding::{bind, digest, verify};
use mintswarm_rules::exit::{Outcome, Unusable, Verdict, code};

#[test]
fn digest_matches_the_sha256_vectors() {
    assert_eq!(
        digest(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn a_binding_verifies_its_own_bytes_only() {
    let bound = bind("plan.md", b"abc");
    assert_eq!(bound.path, "plan.md");
    assert_eq!(bound.sha256, digest(b"abc"));
    assert!(verify(&bound, b"abc"));
    assert!(!verify(&bound, b"abd"));
}

#[test]
fn a_wrong_digest_does_not_verify() {
    let mut bound = bind("plan.md", b"abc");
    bound.sha256 = "0".repeat(64);
    assert!(!verify(&bound, b"abc"));
}

#[test]
fn outcomes_map_to_the_three_exit_codes() {
    let clean: Outcome = Ok(Verdict::Clean);
    let hit: Outcome = Ok(Verdict::Hit);
    let unusable: Outcome = Err(Unusable::new("cannot read the plan"));
    assert_eq!(code(&clean), 0);
    assert_eq!(code(&hit), 1);
    assert_eq!(code(&unusable), 2);
}

#[test]
fn unusable_carries_its_message() {
    let error = Unusable::new("the plan is not UTF-8");
    assert_eq!(error.message(), "the plan is not UTF-8");
    assert_eq!(error.to_string(), "the plan is not UTF-8");
    let boxed: Box<dyn std::error::Error> = Box::new(error);
    assert_eq!(boxed.to_string(), "the plan is not UTF-8");
}
