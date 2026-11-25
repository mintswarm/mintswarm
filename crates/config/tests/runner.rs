//! The runner config: its bounds, defaults and unknown keys.

#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use mintswarm_config::{DEFAULT_WALL_CLOCK, Stage, load_runner_config};
use mintswarm_domain::retry::RetryPolicy;

#[test]
fn poll_interval_bounds() {
    for seconds in [9, 61, 0] {
        let text = format!("state_dir: /var/lib/mintswarm\npoll_interval: {seconds}\n");
        let error = load_runner_config(&text).expect_err("out of bounds");
        assert_eq!(error.path(), "poll_interval", "{seconds}: {error}");
    }
    for value in ["true", "30.5", "'30'"] {
        let text = format!("state_dir: s\npoll_interval: {value}\n");
        let error = load_runner_config(&text).expect_err("not whole seconds");
        assert_eq!(error.path(), "poll_interval", "{value}: {error}");
    }
    for seconds in [10, 30, 60] {
        let text = format!("state_dir: s\npoll_interval: {seconds}\n");
        let config = load_runner_config(&text).expect("within bounds");
        assert_eq!(config.poll_interval(), Duration::from_secs(seconds));
    }
    let config = load_runner_config("state_dir: s\n").expect("loads");
    assert_eq!(config.poll_interval(), Duration::from_secs(30));
}

#[test]
fn runner_unknown_key_names_path() {
    let error = load_runner_config("state_dir: s\nwall_clock: {plan: 60, lint: 5}\n")
        .expect_err("an unknown stage");
    assert_eq!(error.path(), "wall_clock.lint", "{error}");
    let error = load_runner_config("state_dir: s\nextra: 1\n").expect_err("an unknown key");
    assert_eq!(error.path(), "extra", "{error}");
}

#[test]
fn runner_defaults() {
    let config = load_runner_config("state_dir: /var/lib/mintswarm\n").expect("loads");
    assert_eq!(config.state_dir(), Path::new("/var/lib/mintswarm"));
    assert_eq!(config.workers(), 1);
    for stage in Stage::ALL {
        assert_eq!(config.wall_clock(stage), Duration::from_secs(3600));
    }
    assert_eq!(DEFAULT_WALL_CLOCK, Duration::from_secs(3600));

    let config = load_runner_config(
        "state_dir: s\nworkers: 4\nwall_clock: {implement: 7200, gates: 90.5}\n",
    )
    .expect("loads");
    assert_eq!(config.workers(), 4);
    assert_eq!(
        config.wall_clock(Stage::Implement),
        Duration::from_secs(7200)
    );
    assert_eq!(
        config.wall_clock(Stage::Gates),
        Duration::from_millis(90_500)
    );
    assert_eq!(config.wall_clock(Stage::Review), DEFAULT_WALL_CLOCK);
}

#[test]
fn runner_values_rejected() {
    for (text, path) in [
        ("workers: 1\n", "state_dir"),
        ("state_dir: ''\n", "state_dir"),
        ("state_dir: s\nworkers: 0\n", "workers"),
        ("state_dir: s\nworkers: true\n", "workers"),
        ("state_dir: s\nwall_clock: {plan: 0}\n", "wall_clock.plan"),
        (
            "state_dir: s\nwall_clock: {plan: 1e-300}\n",
            "wall_clock.plan",
        ),
        (
            "state_dir: s\nwall_clock: {plan: '60'}\n",
            "wall_clock.plan",
        ),
        ("state_dir: s\nwall_clock: [plan]\n", "wall_clock"),
        ("[state_dir]\n", ""),
    ] {
        let error = load_runner_config(text).expect_err(text);
        assert_eq!(error.path(), path, "{text}: {error}");
    }
}

#[test]
fn github_retry_defaults_bounds_and_unknown_keys() {
    let config = load_runner_config("state_dir: s\n").expect("loads");
    assert_eq!(config.github_retry(), RetryPolicy::default());

    let config = load_runner_config("state_dir: s\ngithub_retry: {attempts: 6, total: 300}\n")
        .expect("loads");
    let policy = config.github_retry();
    assert_eq!(policy.attempts, 6);
    assert_eq!(policy.total, Duration::from_secs(300));
    assert_eq!(policy.base_delay, RetryPolicy::default().base_delay);
    assert_eq!(policy.max_delay, RetryPolicy::default().max_delay);

    let config = load_runner_config(
        "state_dir: s\ngithub_retry: {attempts: 1, base_delay: 0.5, max_delay: 0.5, total: 2}\n",
    )
    .expect("loads");
    assert_eq!(
        config.github_retry(),
        RetryPolicy {
            attempts: 1,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_millis(500),
            total: Duration::from_secs(2),
        }
    );

    for (retry, path) in [
        ("{attempts: 0}", "github_retry.attempts"),
        ("{attempts: 1.5}", "github_retry.attempts"),
        ("{attempts: 4294967296}", "github_retry.attempts"),
        ("{base_delay: 0}", "github_retry.base_delay"),
        ("{max_delay: '20'}", "github_retry.max_delay"),
        ("{total: -1}", "github_retry.total"),
        ("{base_delay: 5, max_delay: 2}", "github_retry.max_delay"),
        // A base delay above the default cap needs a cap of its own.
        ("{base_delay: 30}", "github_retry.max_delay"),
        ("{retries: 3}", "github_retry.retries"),
        ("[attempts]", "github_retry"),
    ] {
        let text = format!("state_dir: s\ngithub_retry: {retry}\n");
        let error = load_runner_config(&text).expect_err(&text);
        assert_eq!(error.path(), path, "{text}: {error}");
    }
}
