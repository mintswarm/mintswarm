//! Drives the health routes of `serve` in-process, and its refusal of a listen address that is
//! not loopback.

use std::net::SocketAddr;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mintswarm_config::load_plane_config;
use mintswarm_core::plane::Readiness;
use mintswarm_server::serve::{ServeError, check_listen, router, run};
use tokio::sync::watch;
use tower::ServiceExt;

/// Every combination of the three checks.
fn every_state() -> Vec<Readiness> {
    let mut states = Vec::new();
    for leader in [false, true] {
        for database in [false, true] {
            for migrations in [false, true] {
                states.push(Readiness {
                    leader,
                    database,
                    migrations,
                });
            }
        }
    }
    states
}

/// The status and body of `GET <path>` with `state`.
async fn get(state: Readiness, path: &str) -> (StatusCode, String) {
    let (_tx, rx) = watch::channel(state);
    let response = router(rx)
        .oneshot(
            Request::get(path)
                .body(Body::empty())
                .expect("the request builds"),
        )
        .await
        .expect("the router answers");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("the body reads");
    (
        status,
        String::from_utf8(body.to_vec()).expect("the body is UTF-8"),
    )
}

#[tokio::test]
async fn healthz_answers_200_in_every_state() {
    for state in every_state() {
        let (status, _) = get(state, "/healthz").await;
        assert_eq!(status, StatusCode::OK, "{state:?}");
    }
}

#[tokio::test]
async fn readyz_answers_503_naming_each_failing_check() {
    for state in every_state() {
        let (status, body) = get(state, "/readyz").await;
        if state.is_ready() {
            assert_eq!(status, StatusCode::OK, "{state:?}");
            continue;
        }
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{state:?}");
        for (name, holds) in [
            ("leader", state.leader),
            ("database", state.database),
            ("migrations", state.migrations),
        ] {
            assert_eq!(body.contains(name), !holds, "{state:?}: {body}");
        }
    }
}

#[test]
fn only_loopback_addresses_are_accepted() {
    for addr in ["0.0.0.0:0", "[::]:0", "203.0.113.1:0"] {
        let addr: SocketAddr = addr.parse().expect("an address");
        assert!(
            matches!(check_listen(addr), Err(ServeError::NoAuthProvider)),
            "{addr} is refused"
        );
    }
    for addr in ["127.0.0.1:0", "[::1]:0"] {
        let addr: SocketAddr = addr.parse().expect("an address");
        assert!(check_listen(addr).is_ok(), "{addr} is accepted");
    }
}

#[tokio::test]
async fn run_refuses_a_non_loopback_address_before_connecting() {
    let config = load_plane_config(
        "listen: 0.0.0.0:0\ndatabase:\n  url: postgres://127.0.0.1:1/mintswarm\n",
        |_| None,
    )
    .expect("the settings load");
    let result = run(config, std::future::pending()).await;
    assert!(
        matches!(result, Err(ServeError::NoAuthProvider)),
        "{result:?}"
    );
}
