//! Retrying `git fetch`: wrappers around git on the `file://` fixture fail the fetch as a test
//! asks, printing only our own constants or nothing, and the tests check classes, attempt counts
//! and the stored record, never git's wording.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output};
use std::time::Duration;

use mintswarm_domain::plane::Credential;
use mintswarm_domain::retry::RetryPolicy;
use mintswarm_domain::vcs::{RefOrId, Vcs};

use super::super::retry::{NETWORK_MARKERS, UNKNOWN_ATTEMPTS, run};
use super::super::{FetchFailure, GitError, GitStore};
use super::{Fixture, script};
use crate::exec::store::{NewUnrecognised, OutboxMode, Store, SystemClock};

/// Four attempts with no waits.
fn quick() -> RetryPolicy {
    RetryPolicy {
        attempts: 4,
        base_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
        ..RetryPolicy::default()
    }
}

/// A store whose git is a wrapper that counts the fetches it is asked for. The first `fails`
/// fetches print `stderr` and exit with `code`; every other command, and every later fetch,
/// runs git. Returns the store and the count file.
fn wrapped(fx: &Fixture, name: &str, fails: u32, code: i32, stderr: &str) -> (GitStore, PathBuf) {
    assert!(!stderr.contains('\''), "the message is single-quoted");
    let count = fx.dir.path().join(format!("{name}.count"));
    let wrapper = fx.dir.path().join(format!("{name}-git"));
    script(
        &wrapper,
        &format!(
            "f=\nfor a in \"$@\"; do [ \"$a\" = fetch ] && f=1; done\n\
             if [ -n \"$f\" ]; then\n\
             echo x >> '{count}'\n\
             if [ \"$(wc -l < '{count}')\" -le {fails} ]; then printf '%s' '{stderr}' >&2; exit {code}; fi\n\
             fi\n\
             exec git \"$@\"",
            count = count.display(),
        ),
    );
    let mut store = fx.store(name);
    store.set_program(&wrapper);
    store.set_retry_policy(quick());
    (store, count)
}

/// How many fetches the wrapper was asked for.
fn fetches(count: &Path) -> usize {
    std::fs::read_to_string(count).map_or(0, |text| text.lines().count())
}

fn side() -> [RefOrId; 1] {
    [RefOrId::Ref("refs/heads/side".to_owned())]
}

#[test]
fn fetch_retries_a_network_failure_then_succeeds() {
    let fx = Fixture::new();
    let (store, count) = wrapped(&fx, "network", 2, 128, NETWORK_MARKERS[0]);
    let report = store.fetch(&fx.remote(), &side(), None).unwrap();
    assert_eq!(report.refs.len(), 1);
    assert_eq!(
        report.refs[0].name,
        "refs/mintswarm/fetched/origin/heads/side"
    );
    assert_eq!(report.refs[0].id, fx.side);
    assert_eq!(fetches(&count), 3);
}

#[test]
fn fetch_gives_up_when_attempts_run_out() {
    let fx = Fixture::new();
    let (store, count) = wrapped(&fx, "exhausted", u32::MAX, 128, NETWORK_MARKERS[0]);
    let err = store.fetch(&fx.remote(), &side(), None).unwrap_err();
    assert!(
        matches!(
            err,
            GitError::FetchExhausted {
                attempts: 4,
                last: FetchFailure::Network,
                unrecognised: None,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(fetches(&count), 4);
}

#[test]
fn fetch_stops_an_unknown_failure_at_the_smaller_cap() {
    let fx = Fixture::new();
    let (store, count) = wrapped(&fx, "unknown", u32::MAX, 128, "");
    let err = store.fetch(&fx.remote(), &side(), None).unwrap_err();
    match err {
        GitError::FetchExhausted {
            attempts,
            last,
            unrecognised,
            ..
        } => {
            assert_eq!(attempts, UNKNOWN_ATTEMPTS);
            assert_eq!(last, FetchFailure::Unknown);
            assert!(unrecognised.is_some());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fetches(&count), 2);
}

#[test]
fn fetch_retries_an_unknown_failure_once() {
    let fx = Fixture::new();
    let (store, count) = wrapped(&fx, "unknown-once", 1, 128, "");
    let report = store.fetch(&fx.remote(), &side(), None).unwrap();
    assert_eq!(report.refs[0].id, fx.side);
    assert_eq!(fetches(&count), 2);
}

#[test]
fn fetch_fails_at_once_on_a_missing_id() {
    let fx = Fixture::new();
    let (store, count) = wrapped(&fx, "missing", 0, 128, "");
    let absent = RefOrId::Id(mintswarm_domain::ids::RevId(super::GITLINK.to_owned()));
    let err = store.fetch(&fx.remote(), &[absent], None).unwrap_err();
    assert!(matches!(err, GitError::Git { .. }), "{err:?}");
    assert_eq!(fetches(&count), 1);
}

#[test]
fn fetch_does_not_retry_other_exit_codes() {
    let fx = Fixture::new();
    let (store, count) = wrapped(&fx, "exit-1", u32::MAX, 1, "");
    let err = store.fetch(&fx.remote(), &side(), None).unwrap_err();
    assert!(matches!(err, GitError::Git { .. }), "{err:?}");
    assert_eq!(fetches(&count), 1);
}

/// An output with exit code 128 that wrote `stderr`.
fn failed(stderr: &str) -> Output {
    Output {
        status: ExitStatus::from_raw(128 << 8),
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

/// The waits `run` asks for under `policy`, with every attempt a network failure and jitter
/// 1.0, and the attempts it made.
fn waits(policy: &RetryPolicy) -> (Vec<Duration>, u32) {
    let mut slept = Vec::new();
    let mut calls = 0;
    let err = run(
        policy,
        None,
        || {
            calls += 1;
            Ok(failed(NETWORK_MARKERS[0]))
        },
        || 1.0,
        |wait| slept.push(wait),
    )
    .unwrap_err();
    let GitError::FetchExhausted { attempts, last, .. } = err else {
        panic!("{err:?}");
    };
    assert_eq!(last, FetchFailure::Network);
    assert_eq!(attempts, calls);
    (slept, attempts)
}

#[test]
fn waits_are_jittered_ceilings_within_the_total() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.attempts, 4);
    let (slept, attempts) = waits(&policy);
    assert_eq!(slept, [1, 2, 4].map(Duration::from_secs));
    assert_eq!(attempts, 4);

    let short = RetryPolicy {
        total: Duration::from_millis(1500),
        ..policy
    };
    let (slept, attempts) = waits(&short);
    assert_eq!(slept, [Duration::from_secs(1)]);
    assert_eq!(attempts, 2);
}

/// Our own wording of a failure git might write: a URL with the credential, an id and a
/// number.
fn own_unknown(token: &str) -> String {
    format!(
        "mintswarm test failure at https://x-access-token:{token}@example.invalid/r \
         on 0123456789abcdef0123456789abcdef01234567 after 42 tries"
    )
}

#[test]
fn an_exhausted_unknown_failure_is_recorded_without_secrets() {
    let fx = Fixture::new();
    let token = format!("msw-test-token-{}", std::process::id());
    let message = own_unknown(&token);
    let (store, count) = wrapped(&fx, "recorded", u32::MAX, 128, &message);
    let cred = Credential {
        token: token.clone(),
        expires_at: "2026-11-25T01:00:00Z".to_owned(),
        permissions: vec!["contents:read".to_owned()],
    };
    let record = || {
        let err = store.fetch(&fx.remote(), &side(), Some(&cred)).unwrap_err();
        let GitError::FetchExhausted {
            last: FetchFailure::Unknown,
            unrecognised: Some(record),
            ..
        } = err
        else {
            panic!("{err:?}");
        };
        record
    };
    let first = record();
    for secret in [token.as_str(), "x-access-token", "example.invalid"] {
        assert!(
            !first.excerpt.contains(secret),
            "{secret} is in the excerpt"
        );
    }
    assert!(first.excerpt.contains("[url]"));
    assert!(first.excerpt.contains("[id]"));
    let second = record();
    assert_eq!(second.signature, first.signature);
    assert_eq!(fetches(&count), 2 * UNKNOWN_ATTEMPTS as usize);

    let mut runner = Store::open(
        &fx.dir.path().join("runner-state"),
        SystemClock,
        OutboxMode::Suppressed,
    )
    .unwrap();
    for failure in [&first, &second] {
        let new = NewUnrecognised {
            adapter: "git".to_owned(),
            signature: failure.signature.clone(),
            excerpt: failure.excerpt.clone(),
            status: failure.status.clone(),
        };
        runner
            .transaction(|tx| tx.record_unrecognised(&new))
            .unwrap();
    }
    let rows = runner.transaction(|tx| tx.unrecognised("git")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].count, 2);
    for column in [
        &rows[0].adapter,
        &rows[0].signature,
        &rows[0].excerpt,
        &rows[0].status,
    ] {
        assert!(!column.contains(&token), "the token is stored");
    }
}
