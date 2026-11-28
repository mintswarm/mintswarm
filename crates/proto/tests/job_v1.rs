//! Parity of `mintswarm.job.v1` with `domain::job`: every variant of each
//! same-named domain enum has the generated value named with the type prefix
//! followed by the variant's name in upper snake case, and the proto enum has
//! no other value besides `<PREFIX>_UNSPECIFIED = 0`.
//!
//! Temporary: job-in-proto removes this test, and with it this test's use of
//! the `protox` dev-dependency. `mintswarm-domain` stays a normal dependency,
//! for `otlp`.

use std::collections::BTreeSet;
use std::path::Path;

use mintswarm_proto::prost_types::EnumDescriptorProto;

/// The vocabulary file, relative to `api/proto`.
const JOB_PROTO: &str = "mintswarm/job/v1/job.proto";

/// The five enums the package holds.
const VOCABULARY: [&str; 5] = [
    "JobKind",
    "JobState",
    "Event",
    "EscalationCause",
    "AssignReason",
];

/// `CamelCase` to `UPPER_SNAKE_CASE`: an `_` before each uppercase letter
/// after the first, then everything uppercased.
fn upper_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 8);
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// Compiles `job.proto` and returns its file descriptor's message count and
/// enum descriptors.
fn job_file() -> (usize, Vec<EnumDescriptorProto>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/proto");
    let fds = protox::compile([root.join(JOB_PROTO)], [&root]).expect("job.proto compiles");
    let file = fds
        .file
        .into_iter()
        .find(|file| file.name() == JOB_PROTO)
        .expect("the descriptor set holds job.proto");
    (file.message_type.len(), file.enum_type)
}

/// The enum descriptor named `name` in `job.proto`.
fn job_enum(name: &str) -> EnumDescriptorProto {
    job_file()
        .1
        .into_iter()
        .find(|e| e.name() == name)
        .unwrap_or_else(|| panic!("job.proto defines {name}"))
}

/// A test that `mintswarm_proto::job::v1::$name` matches
/// `mintswarm_domain::job::$name` in both directions.
macro_rules! parity {
    ($test:ident, $name:ident) => {
        #[test]
        fn $test() {
            let prefix = upper_snake(stringify!($name));
            let unspecified = format!("{prefix}_UNSPECIFIED");

            let mut expected = BTreeSet::new();
            for variant in mintswarm_domain::job::$name::ALL {
                let value = format!("{prefix}_{}", upper_snake(&format!("{variant:?}")));
                let generated = mintswarm_proto::job::v1::$name::from_str_name(&value)
                    .unwrap_or_else(|| panic!("no generated value {value} for {variant:?}"));
                assert_eq!(generated.as_str_name(), value);
                expected.insert(value);
            }
            expected.insert(unspecified.clone());

            let descriptor = job_enum(stringify!($name));
            let actual: BTreeSet<String> = descriptor
                .value
                .iter()
                .map(|value| value.name().to_owned())
                .collect();
            assert_eq!(actual, expected);

            let zero = descriptor
                .value
                .iter()
                .find(|value| value.number() == 0)
                .expect("a value numbered 0");
            assert_eq!(zero.name(), unspecified);
        }
    };
}

parity!(job_kind_matches_the_domain, JobKind);
parity!(job_state_matches_the_domain, JobState);
parity!(event_matches_the_domain, Event);
parity!(escalation_cause_matches_the_domain, EscalationCause);
parity!(assign_reason_matches_the_domain, AssignReason);

#[test]
fn package_holds_only_the_vocabulary() {
    let (messages, enums) = job_file();
    assert_eq!(messages, 0, "job.proto defines no messages");

    let names: BTreeSet<&str> = enums.iter().map(EnumDescriptorProto::name).collect();
    assert_eq!(names, BTreeSet::from(VOCABULARY));
    assert!(
        names.iter().all(|name| !name.contains("AttemptOutcome")),
        "the attempt outcome is runner.v1's AttemptOutcomeKind"
    );
}

#[test]
fn upper_snake_splits_before_each_capital() {
    assert_eq!(
        upper_snake("AwaitingDesignApproval"),
        "AWAITING_DESIGN_APPROVAL"
    );
    assert_eq!(upper_snake("JobState"), "JOB_STATE");
    assert_eq!(upper_snake("Done"), "DONE");
}
