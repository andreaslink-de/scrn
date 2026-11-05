//! Integration tests for `GET /healthz` against a temporary SQLite database.

// Test helpers outside #[test] functions may unwrap as well.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use scrn_server::{Config, app, healthcheck, init_state, serve};
use tower::ServiceExt;

fn test_config(dir: &std::path::Path) -> Config {
    let data_dir = dir.to_string_lossy().into_owned();
    Config::from_lookup(|name| match name {
        "SCRN_DATA_DIR" => Some(data_dir.clone()),
        "SCRN_BIND" => Some("127.0.0.1:0".to_owned()),
        "SCRN_COOKIE_SECURE" => Some("false".to_owned()),
        _ => None,
    })
    .unwrap()
}

#[tokio::test]
async fn healthz_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let state = init_state(test_config(dir.path())).await.unwrap();

    let response = app(state)
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"ok\n");
}

#[tokio::test]
async fn init_creates_database_and_files_dir() {
    let dir = tempfile::tempdir().unwrap();
    init_state(test_config(dir.path())).await.unwrap();
    assert!(dir.path().join("scrn.db").is_file());
    assert!(dir.path().join("files").is_dir());
}

#[tokio::test]
async fn healthz_returns_503_when_database_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let state = init_state(test_config(dir.path())).await.unwrap();
    state.db.close().await;

    let response = app(state)
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn unknown_route_returns_404() {
    let dir = tempfile::tempdir().unwrap();
    let state = init_state(test_config(dir.path())).await.unwrap();

    let response = app(state)
        .oneshot(Request::get("/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn probe_against_running_server_and_graceful_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let state = init_state(test_config(dir.path())).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(listener, state, async {
        let _ = rx.await;
    }));

    healthcheck::probe(addr).await.unwrap();

    tx.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(healthcheck::probe(addr).await.is_err());
}
