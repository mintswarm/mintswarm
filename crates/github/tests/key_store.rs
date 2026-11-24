//! Each key-store backend returns the value through its stdout pipe, never in argv, against fake
//! backend CLIs.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use mintswarm_github::{KeyStore, KeyStoreError, read_key};
use support::{Fakes, Home, MARKER, MARKER_BASE64};

const SERVICE: &str = "mintswarm-dev";
const ACCOUNT: &str = "Iv23liTest";

fn assert_not_in_argv(fakes: &Fakes) {
    for call in fakes.calls() {
        assert!(!call.contains(MARKER), "the value is in argv: {call}");
        assert!(
            !call.contains(MARKER_BASE64),
            "the value is in argv: {call}"
        );
    }
}

async fn read(store: KeyStore, fakes: &Fakes, home: &Home) -> Result<Vec<u8>, KeyStoreError> {
    read_key(store, SERVICE, ACCOUNT, &fakes.tools, &home.env)
        .await
        .map(|value| value.expose().to_vec())
}

#[tokio::test]
async fn keychain_decodes_the_value_from_its_pipe() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    assert_eq!(
        read(KeyStore::Keychain, &fakes, &home).await.unwrap(),
        MARKER.as_bytes()
    );
    assert_eq!(
        fakes.calls(),
        [format!(
            "security find-generic-password -s {SERVICE} -a {ACCOUNT} -w"
        )]
    );
    assert_not_in_argv(&fakes);
}

#[tokio::test]
async fn keychain_refuses_a_value_that_is_not_base64() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    fakes.write("security", "printf 'not base64!'");
    let error = read(KeyStore::Keychain, &fakes, &home).await.unwrap_err();
    assert!(matches!(error, KeyStoreError::Decode { .. }), "{error}");
}

#[tokio::test]
async fn secret_service_reads_its_pipe() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    let value = read(KeyStore::SecretService, &fakes, &home).await.unwrap();
    assert_eq!(value, MARKER.as_bytes());
    assert_eq!(
        fakes.calls(),
        [format!(
            "secret-tool lookup service {SERVICE} account {ACCOUNT}"
        )]
    );
    assert_not_in_argv(&fakes);
}

#[tokio::test]
async fn every_backend_that_can_prompt_is_stopped_after_the_timeout() {
    for (store, name) in [
        (KeyStore::Keychain, "security"),
        (KeyStore::SecretService, "secret-tool"),
        (KeyStore::Pass, "pass"),
        (KeyStore::SystemdCreds, "systemd-creds"),
    ] {
        let (fakes, mut home) = (Fakes::new(false), Home::new());
        fakes.write(name, "exec sleep 10");
        home.env.timeout = Duration::from_secs(1);
        let credstore = home.config_dir().join("credstore");
        std::fs::create_dir_all(&credstore).unwrap();
        std::fs::write(credstore.join(format!("{SERVICE}@{ACCOUNT}.cred")), b"x").unwrap();
        let started = Instant::now();
        let error = read(store, &fakes, &home).await.unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{name}: {:?}",
            started.elapsed()
        );
        assert!(
            matches!(error, KeyStoreError::TimedOut { .. }),
            "{name}: {error}"
        );
        let message = error.to_string();
        assert!(message.contains(name), "{message}");
        assert!(message.contains("store-credentials.sh rotate"), "{message}");
    }
}

#[tokio::test]
async fn a_timeout_also_stops_the_processes_a_backend_started() {
    let (fakes, mut home) = (Fakes::new(false), Home::new());
    let pidfile = fakes.dir.path().join("helper.pid");
    // The helper stands in for a pinentry: a grandchild that would outlive its parent.
    fakes.write(
        "pass",
        &format!(
            "sleep 30 &\nprintf '%s' \"$!\" > '{}'\nwait",
            pidfile.display()
        ),
    );
    // Generous, so the fake (already warmed up by `Fakes::write`) still starts its helper under a
    // loaded, parallel test run.
    home.env.timeout = Duration::from_secs(4);
    let error = read(KeyStore::Pass, &fakes, &home).await.unwrap_err();
    assert!(matches!(error, KeyStoreError::TimedOut { .. }), "{error}");
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .expect("the fake did not start its helper before the timeout")
        .trim()
        .parse()
        .unwrap();
    let pid = rustix::process::Pid::from_raw(pid).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while rustix::process::test_kill_process(pid).is_ok() {
        assert!(
            Instant::now() < deadline,
            "the helper {pid:?} is still running"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn pass_reads_its_pipe() {
    let (fakes, mut home) = (Fakes::new(false), Home::new());
    home.env.password_store_dir = Some(home.dir.path().join("store"));
    fakes.write(
        "pass",
        &format!(
            "[ \"$PASSWORD_STORE_DIR\" = '{}' ] || exit 3\nprintf '%s' '{MARKER}'",
            home.dir.path().join("store").display()
        ),
    );
    assert_eq!(
        read(KeyStore::Pass, &fakes, &home).await.unwrap(),
        MARKER.as_bytes()
    );
    assert_eq!(
        fakes.calls(),
        [format!("pass show mintswarm/{SERVICE}/{ACCOUNT}")]
    );
    assert_not_in_argv(&fakes);
}

#[tokio::test]
async fn systemd_creds_decrypts_the_credstore_file_from_its_pipe() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    let credstore = home.config_dir().join("credstore");
    std::fs::create_dir_all(&credstore).unwrap();
    let cred = credstore.join(format!("{SERVICE}@{ACCOUNT}.cred"));
    std::fs::write(&cred, b"encrypted").unwrap();
    let value = read(KeyStore::SystemdCreds, &fakes, &home).await.unwrap();
    assert_eq!(value, MARKER.as_bytes());
    assert_eq!(
        fakes.calls(),
        [
            String::from("systemd-creds --version"),
            format!(
                "systemd-creds --user decrypt --name={SERVICE}@{ACCOUNT} {} -",
                cred.display()
            ),
        ]
    );
    assert_not_in_argv(&fakes);
}

#[tokio::test]
async fn systemd_creds_as_root_skips_the_version_check_and_user() {
    let (fakes, mut home) = (Fakes::new(false), Home::new());
    home.env.is_root = true;
    let credstore = home.config_dir().join("credstore");
    std::fs::create_dir_all(&credstore).unwrap();
    let cred = credstore.join(format!("{SERVICE}@{ACCOUNT}.cred"));
    std::fs::write(&cred, b"encrypted").unwrap();
    read(KeyStore::SystemdCreds, &fakes, &home).await.unwrap();
    assert_eq!(
        fakes.calls(),
        [format!(
            "systemd-creds decrypt --name={SERVICE}@{ACCOUNT} {} -",
            cred.display()
        )]
    );
}

#[tokio::test]
async fn systemd_creds_refuses_systemd_before_256() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    fakes.write("systemd-creds", "echo 'systemd 255 (255.4)'");
    let credstore = home.config_dir().join("credstore");
    std::fs::create_dir_all(&credstore).unwrap();
    std::fs::write(credstore.join(format!("{SERVICE}@{ACCOUNT}.cred")), b"x").unwrap();
    let error = read(KeyStore::SystemdCreds, &fakes, &home)
        .await
        .unwrap_err();
    assert!(matches!(error, KeyStoreError::SystemdUnusable), "{error}");
    assert_eq!(fakes.calls(), ["systemd-creds --version"]);
}

#[tokio::test]
async fn systemd_creds_reads_the_credentials_directory_in_process() {
    let (fakes, mut home) = (Fakes::new(false), Home::new());
    let passed = home.dir.path().join("credentials");
    std::fs::create_dir_all(&passed).unwrap();
    std::fs::write(passed.join(format!("{SERVICE}@{ACCOUNT}")), MARKER).unwrap();
    home.env.credentials_directory = Some(passed);
    let value = read(KeyStore::SystemdCreds, &fakes, &home).await.unwrap();
    assert_eq!(value, MARKER.as_bytes());
    assert!(fakes.calls().is_empty(), "{:?}", fakes.calls());
}

#[tokio::test]
async fn systemd_creds_reports_a_missing_credential() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    let error = read(KeyStore::SystemdCreds, &fakes, &home)
        .await
        .unwrap_err();
    assert!(matches!(error, KeyStoreError::NotFound(_)), "{error}");
    assert!(fakes.calls().is_empty());
}

#[tokio::test]
async fn file_reads_in_process_after_the_checks() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    home.put_file_key(SERVICE, ACCOUNT, MARKER.as_bytes());
    assert_eq!(
        read(KeyStore::File, &fakes, &home).await.unwrap(),
        MARKER.as_bytes()
    );
    // Only the work-tree check runs a process, and the value is never near it.
    assert_eq!(fakes.calls(), ["git rev-parse --is-inside-work-tree"]);
    assert_not_in_argv(&fakes);
}

async fn assert_refused(fakes: &Fakes, home: &Home, needle: &str) {
    let error = read(KeyStore::File, fakes, home).await.unwrap_err();
    assert!(matches!(error, KeyStoreError::Refused { .. }), "{error}");
    assert!(error.to_string().contains(needle), "{error}");
}

#[tokio::test]
async fn file_refuses_a_key_directory_in_a_work_tree() {
    let (fakes, home) = (Fakes::new(true), Home::new());
    home.put_file_key(SERVICE, ACCOUNT, MARKER.as_bytes());
    assert_refused(&fakes, &home, "git work tree").await;
}

#[tokio::test]
async fn file_refuses_a_wide_key_file() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    let file = home.put_file_key(SERVICE, ACCOUNT, MARKER.as_bytes());
    for mode in [0o640, 0o604, 0o610] {
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        assert_refused(&fakes, &home, "group or other").await;
    }
}

#[tokio::test]
async fn file_refuses_a_wide_key_directory() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    home.put_file_key(SERVICE, ACCOUNT, MARKER.as_bytes());
    let keys = home.config_dir().join("keys");
    std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o750)).unwrap();
    assert_refused(&fakes, &home, "group or other").await;
}

#[tokio::test]
async fn file_refuses_a_symlinked_key_file() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    let file = home.put_file_key(SERVICE, ACCOUNT, MARKER.as_bytes());
    let real = home.config_dir().join("keys").join("real");
    std::fs::rename(&file, &real).unwrap();
    std::os::unix::fs::symlink(&real, &file).unwrap();
    assert_refused(&fakes, &home, "symlink").await;
}

#[tokio::test]
async fn file_refuses_a_symlinked_key_directory() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    home.put_file_key(SERVICE, ACCOUNT, MARKER.as_bytes());
    let keys = home.config_dir().join("keys");
    let real = home.config_dir().join("real-keys");
    std::fs::rename(&keys, &real).unwrap();
    std::os::unix::fs::symlink(&real, &keys).unwrap();
    assert_refused(&fakes, &home, "symlink").await;
}

#[tokio::test]
async fn file_refuses_symlinked_config_directories() {
    for link in [".config", ".config/mintswarm"] {
        let (fakes, home) = (Fakes::new(false), Home::new());
        home.put_file_key(SERVICE, ACCOUNT, MARKER.as_bytes());
        let path = home.dir.path().join(link);
        let real = home.dir.path().join("real");
        std::fs::rename(&path, &real).unwrap();
        std::os::unix::fs::symlink(&real, &path).unwrap();
        assert_refused(&fakes, &home, "dotfiles").await;
    }
}

#[tokio::test]
async fn file_refuses_a_key_that_is_not_a_regular_file() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    let file = home.put_file_key(SERVICE, ACCOUNT, MARKER.as_bytes());
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_refused(&fakes, &home, "not a regular file").await;
}

#[tokio::test]
async fn file_reports_a_missing_key() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    let error = read(KeyStore::File, &fakes, &home).await.unwrap_err();
    assert!(matches!(error, KeyStoreError::NotFound(_)), "{error}");
}

#[tokio::test]
async fn every_backend_refuses_an_empty_value() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    for name in ["security", "secret-tool", "pass"] {
        fakes.write(name, "true");
    }
    for store in [KeyStore::Keychain, KeyStore::SecretService, KeyStore::Pass] {
        let error = read(store, &fakes, &home).await.unwrap_err();
        assert!(
            matches!(error, KeyStoreError::Empty { .. }),
            "{store:?}: {error}"
        );
    }
    home.put_file_key(SERVICE, ACCOUNT, b"");
    let error = read(KeyStore::File, &fakes, &home).await.unwrap_err();
    assert!(matches!(error, KeyStoreError::Empty { .. }), "{error}");
}

#[tokio::test]
async fn a_failing_backend_carries_no_output() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    fakes.write(
        "pass",
        &format!("echo 'stderr {MARKER}' >&2\necho '{MARKER}'\nexit 1"),
    );
    let error = read(KeyStore::Pass, &fakes, &home).await.unwrap_err();
    assert!(matches!(error, KeyStoreError::Failed { .. }), "{error}");
    assert!(!error.to_string().contains(MARKER), "{error}");
    assert!(!format!("{error:?}").contains(MARKER), "{error:?}");
}

#[tokio::test]
async fn invalid_names_are_refused_before_any_backend_runs() {
    let (fakes, home) = (Fakes::new(false), Home::new());
    for (service, account) in [("-s", ACCOUNT), (SERVICE, "a b"), ("", ACCOUNT)] {
        let error = read_key(KeyStore::Pass, service, account, &fakes.tools, &home.env)
            .await
            .unwrap_err();
        assert!(
            matches!(error, KeyStoreError::InvalidName { .. }),
            "{error}"
        );
    }
    assert!(fakes.calls().is_empty());
}
