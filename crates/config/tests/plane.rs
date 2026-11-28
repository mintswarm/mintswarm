//! The control plane's `mintswarm.yaml`: its keys and defaults, the environment overrides and
//! their `_FILE` forms, and that neither the database URL nor the password is ever shown.

#![cfg(test)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

use mintswarm_config::{
    DATABASE_PASSWORD_ENV, DATABASE_URL_ENV, Error, Host, LISTEN_ENV, PlaneConfig, ProjectEntry,
    load_plane_config,
};
use mintswarm_domain::ids::ProjectId;

const PLANTED: &str = "s3cr3t-plant";

const FULL: &str = "\
listen: 127.0.0.1:8080
database:
  url: postgres://mintswarm@db:5432/mintswarm
  password: from-the-file
projects:
  - project_id: mintswarm
    repo: mintswarm/mintswarm
    host: github
    installation_id: 4242
    weight: 3
  - project_id: landings
    repo: mintswarm/landings
    installation_id: 4242
";

const MINIMAL: &str = "\
listen: 127.0.0.1:8080
database:
  url: postgres://db/mintswarm
";

/// A lookup over a fixed set of variables.
fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
    let vars: BTreeMap<String, OsString> = vars
        .iter()
        .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
        .collect();
    move |name| vars.get(name).cloned()
}

fn no_env() -> impl Fn(&str) -> Option<OsString> {
    |_| None
}

fn load(text: &str) -> Result<PlaneConfig, Error> {
    load_plane_config(text, no_env())
}

/// A directory of this test's own under Cargo's temporary directory, emptied first.
fn dir(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("plane-config")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the test directory is created");
    dir
}

fn schema_error_at(result: Result<PlaneConfig, Error>, path: &str) -> Error {
    let error = result.expect_err("the config is refused");
    assert!(matches!(error, Error::Schema { .. }), "{error:?}");
    assert_eq!(error.path(), path, "{error}");
    error
}

#[test]
fn full_document_loads() {
    let config = load(FULL).expect("loads");
    assert_eq!(
        config.listen,
        "127.0.0.1:8080".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(
        config.database.url.expose(),
        "postgres://mintswarm@db:5432/mintswarm"
    );
    assert_eq!(
        config.database.password.as_ref().map(|p| p.expose()),
        Some("from-the-file")
    );
    assert_eq!(
        config.projects,
        vec![
            ProjectEntry {
                project_id: ProjectId("mintswarm".to_owned()),
                repo: "mintswarm/mintswarm".to_owned(),
                host: Host::Github,
                installation_id: 4242,
                weight: 3,
            },
            ProjectEntry {
                project_id: ProjectId("landings".to_owned()),
                repo: "mintswarm/landings".to_owned(),
                host: Host::Github,
                installation_id: 4242,
                weight: 1,
            },
        ]
    );
}

#[test]
fn minimal_document_has_no_password_and_no_projects() {
    let config = load(MINIMAL).expect("loads");
    assert_eq!(config.database.password, None);
    assert!(config.projects.is_empty());
}

#[test]
fn unknown_keys_are_refused() {
    schema_error_at(load(&format!("{MINIMAL}extra: 1\n")), "extra");
    schema_error_at(
        load("listen: 127.0.0.1:1\ndatabase:\n  url: u\n  user: x\n"),
        "database.user",
    );
    schema_error_at(
        load(&format!(
            "{MINIMAL}projects:\n  - project_id: a\n    repo: o/n\n    installation_id: 1\n    mode: active\n"
        )),
        "projects[0].mode",
    );
}

#[test]
fn required_keys_are_checked() {
    schema_error_at(load("database:\n  url: u\n"), "listen");
    schema_error_at(load("listen: 127.0.0.1:1\n"), "database.url");
    schema_error_at(load("listen: 127.0.0.1:1\ndatabase: {}\n"), "database.url");
    schema_error_at(load("listen: 127.0.0.1\ndatabase:\n  url: u\n"), "listen");
    let base = format!("{MINIMAL}projects:\n");
    schema_error_at(
        load(&format!("{base}  - repo: o/n\n    installation_id: 1\n")),
        "projects[0].project_id",
    );
    schema_error_at(
        load(&format!(
            "{base}  - project_id: a\n    installation_id: 1\n"
        )),
        "projects[0].repo",
    );
    schema_error_at(
        load(&format!("{base}  - project_id: a\n    repo: o/n\n")),
        "projects[0].installation_id",
    );
}

#[test]
fn project_values_are_checked() {
    let entry = |extra: &str| {
        format!(
            "{MINIMAL}projects:\n  - project_id: a\n    repo: o/n\n    installation_id: 1\n{extra}"
        )
    };
    schema_error_at(load(&entry("    host: gitlab\n")), "projects[0].host");
    schema_error_at(load(&entry("    weight: 0\n")), "projects[0].weight");
    schema_error_at(
        load(&entry("    weight: 2147483648\n")),
        "projects[0].weight",
    );
    for repo in ["name", "o/n/x", "/n", "o/"] {
        let text = format!(
            "{MINIMAL}projects:\n  - project_id: a\n    repo: {repo}\n    installation_id: 1\n"
        );
        schema_error_at(load(&text), "projects[0].repo");
    }
    let text =
        format!("{MINIMAL}projects:\n  - project_id: a\n    repo: o/n\n    installation_id: 0\n");
    schema_error_at(load(&text), "projects[0].installation_id");
    let twice = format!(
        "{MINIMAL}projects:\n  - project_id: a\n    repo: o/n\n    installation_id: 1\n  - project_id: a\n    repo: o/m\n    installation_id: 1\n"
    );
    schema_error_at(load(&twice), "projects[1].project_id");
}

#[test]
fn required_keys_given_only_by_overrides_are_enough() {
    let config = load_plane_config(
        "",
        env(&[
            (LISTEN_ENV, "[::1]:9000"),
            (DATABASE_URL_ENV, "postgres://env/mintswarm"),
        ]),
    )
    .expect("loads from the overrides alone");
    assert_eq!(config.listen, "[::1]:9000".parse::<SocketAddr>().unwrap());
    assert_eq!(config.database.url.expose(), "postgres://env/mintswarm");
}

#[test]
fn each_override_beats_the_file() {
    let config = load_plane_config(
        FULL,
        env(&[
            (LISTEN_ENV, "127.0.0.1:9999"),
            (DATABASE_URL_ENV, "postgres://env/db"),
            (DATABASE_PASSWORD_ENV, "from-the-env"),
        ]),
    )
    .expect("loads");
    assert_eq!(
        config.listen,
        "127.0.0.1:9999".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(config.database.url.expose(), "postgres://env/db");
    assert_eq!(
        config.database.password.as_ref().map(|p| p.expose()),
        Some("from-the-env")
    );
}

#[test]
fn each_file_form_is_read_with_one_trailing_newline_removed() {
    let dir = dir("file_forms");
    let write = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).expect("the value file is written");
        path.display().to_string()
    };
    let listen = write("listen", "127.0.0.1:7000\n");
    let url = write("url", "postgres://file/db\n");
    let password = write("password", "from-a-file\n\n");
    let config = load_plane_config(
        FULL,
        env(&[
            ("MINTSWARM_LISTEN_FILE", &listen),
            ("MINTSWARM_DATABASE_URL_FILE", &url),
            ("MINTSWARM_DATABASE_PASSWORD_FILE", &password),
        ]),
    )
    .expect("loads");
    assert_eq!(
        config.listen,
        "127.0.0.1:7000".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(config.database.url.expose(), "postgres://file/db");
    assert_eq!(
        config.database.password.as_ref().map(|p| p.expose()),
        Some("from-a-file\n"),
        "only one trailing newline is removed"
    );
}

#[test]
fn unusable_overrides_are_refused_at_the_variable() {
    let dir = dir("unusable");
    let value_file = dir.join("value");
    std::fs::write(&value_file, "x").expect("written");
    let value_file = value_file.display().to_string();
    let empty_file = dir.join("empty");
    std::fs::write(&empty_file, "\n").expect("written");
    let empty_file = empty_file.display().to_string();
    let missing = dir.join("missing").display().to_string();

    let both = [
        (DATABASE_PASSWORD_ENV, "x"),
        ("MINTSWARM_DATABASE_PASSWORD_FILE", value_file.as_str()),
    ];
    schema_error_at(load_plane_config(FULL, env(&both)), DATABASE_PASSWORD_ENV);
    schema_error_at(
        load_plane_config(FULL, env(&[(DATABASE_URL_ENV, "")])),
        DATABASE_URL_ENV,
    );
    schema_error_at(
        load_plane_config(FULL, env(&[("MINTSWARM_DATABASE_URL_FILE", "")])),
        "MINTSWARM_DATABASE_URL_FILE",
    );
    schema_error_at(
        load_plane_config(FULL, env(&[("MINTSWARM_DATABASE_PASSWORD_FILE", &missing)])),
        "MINTSWARM_DATABASE_PASSWORD_FILE",
    );
    schema_error_at(
        load_plane_config(
            FULL,
            env(&[("MINTSWARM_DATABASE_PASSWORD_FILE", &empty_file)]),
        ),
        "MINTSWARM_DATABASE_PASSWORD_FILE",
    );
    schema_error_at(
        load_plane_config(FULL, env(&[(LISTEN_ENV, "localhost")])),
        LISTEN_ENV,
    );
    schema_error_at(
        load_plane_config(FULL, env(&[("MINTSWARM_LISTEN_FILE", &value_file)])),
        "MINTSWARM_LISTEN_FILE",
    );
}

/// Neither `Display` nor `Debug` of `result`, loaded or refused, holds the planted secret.
fn assert_hidden(result: &Result<PlaneConfig, Error>, case: &str) {
    let shown = match result {
        Ok(config) => format!("{config:?}"),
        Err(error) => format!("{error} {error:?}"),
    };
    assert!(!shown.contains(PLANTED), "{case}: {shown}");
}

#[test]
fn the_url_and_password_are_never_shown() {
    let in_url = format!("listen: 127.0.0.1:1\ndatabase:\n  url: postgres://u:{PLANTED}@db/x\n");
    let result = load(&in_url);
    assert!(result.is_ok());
    assert_hidden(&result, "in the URL");

    let in_password =
        format!("listen: 127.0.0.1:1\ndatabase:\n  url: postgres://db/x\n  password: {PLANTED}\n");
    let result = load(&in_password);
    assert!(result.is_ok());
    assert_hidden(&result, "in database.password");

    let mistyped =
        format!("listen: 127.0.0.1:1\ndatabase:\n  url: postgres://db/x\n  pasword: {PLANTED}\n");
    let result = load(&mistyped);
    assert!(result.is_err());
    assert_hidden(&result, "under a mistyped key");

    let top_level = format!("{MINIMAL}password: {PLANTED}\n");
    let result = load(&top_level);
    assert!(result.is_err());
    assert_hidden(&result, "under a top-level key");

    let unconvertible =
        "listen: 127.0.0.1:1\ndatabase:\n  url: postgres://db/x\n  password: 0b1101\n";
    let result = load(unconvertible);
    let error = result.as_ref().expect_err("yq cannot marshal 0b1101");
    assert_eq!(error.path(), "database.password");
    assert!(!format!("{error} {error:?}").contains("0b1101"), "{error}");

    let not_a_string = format!("listen: 127.0.0.1:1\ndatabase:\n  url: [{PLANTED}]\n");
    let result = load(&not_a_string);
    assert!(result.is_err());
    assert_hidden(&result, "as a sequence");

    let dir = dir("hidden");
    let file = dir.join("password");
    std::fs::write(&file, format!("{PLANTED}\n")).expect("written");
    let file = file.display().to_string();
    let result = load_plane_config(MINIMAL, env(&[("MINTSWARM_DATABASE_PASSWORD_FILE", &file)]));
    assert!(result.is_ok());
    assert_hidden(&result, "in a _FILE file");

    let both = [
        (DATABASE_PASSWORD_ENV, PLANTED),
        ("MINTSWARM_DATABASE_PASSWORD_FILE", file.as_str()),
    ];
    let result = load_plane_config(MINIMAL, env(&both));
    assert!(result.is_err());
    assert_hidden(&result, "with both forms set");
}
