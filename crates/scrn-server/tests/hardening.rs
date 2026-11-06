//! Integration tests for the hardening: security headers, body limits,
//! timeouts, panics, malformed input and rate limits.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use common::{PASSWORD, TestServer, upload};
use scrn_server::hardening::{self, FALLBACK_CSP, HSTS, PERMISSIONS_POLICY, REQUEST_TIMEOUT};
use scrn_server::html::{PAGE_CSP, asset_url};
use scrn_server::repo::users::Role;
use scrn_server::{API_FAILURES_PER_IP, LOGIN_ATTEMPTS_PER_IP, UPLOADS_PER_USER};
use tower::ServiceExt;

/// An incompressible RGB image, so the PNG is larger than the form limit.
fn noisy_png(width: u32, height: u32) -> Vec<u8> {
    let mut state = 0x1234_5678u32;
    let pixels: Vec<u8> = (0..width * height * 3)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state & 0xff) as u8
        })
        .collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    out
}

#[tokio::test]
async fn every_response_carries_security_headers() {
    let server = TestServer::new().await; // https://scrn.test
    let user = server.create_user("alice", Role::User).await;
    let mut browser = server.client("192.0.2.1");
    let mut api = server.api_client(&user).await;

    let checks = [
        (browser.get("/login").await, Some(PAGE_CSP)),
        (browser.get("/no/such/page").await, Some(PAGE_CSP)),
        (browser.get(&asset_url("app.css")).await, Some(FALLBACK_CSP)),
        (
            server.client("192.0.2.2").get("/api/v1/me").await,
            Some(FALLBACK_CSP),
        ),
        (api.get("/api/v1/me").await, Some(FALLBACK_CSP)),
        (browser.get("/healthz").await, Some(FALLBACK_CSP)),
    ];
    for (response, csp) in checks {
        let h = &response.headers;
        assert_eq!(h["x-content-type-options"], "nosniff");
        assert_eq!(h["permissions-policy"], PERMISSIONS_POLICY);
        assert_eq!(h["strict-transport-security"], HSTS);
        assert!(h.contains_key("referrer-policy"));
        if let Some(csp) = csp {
            assert_eq!(h["content-security-policy"], csp);
        }
    }
    let page = browser.get("/login").await;
    assert_eq!(page.headers["x-frame-options"], "DENY");
    assert_eq!(page.headers["cross-origin-opener-policy"], "same-origin");
    assert!(!page.body.contains("<script>"));
    assert!(!page.body.contains(" on"));
}

#[tokio::test]
async fn no_hsts_over_plain_http() {
    let server = TestServer::with_env(&[("SCRN_BASE_URL", "http://localhost:8080")]).await;
    let page = server.client("192.0.2.1").get("/login").await;
    assert!(!page.headers.contains_key("strict-transport-security"));
    assert_eq!(page.headers["content-security-policy"], PAGE_CSP);
}

#[tokio::test]
async fn forms_are_size_limited_but_uploads_are_not() {
    let server = TestServer::new().await;
    server.create_user("alice", Role::User).await;
    let mut browser = server.client("192.0.2.1");
    let csrf = browser.get("/login").await.csrf();
    let huge = "x".repeat(hardening::FORM_BODY_LIMIT + 1);
    let response = browser
        .post(
            "/login",
            &[("csrf", &csrf), ("username", "alice"), ("password", &huge)],
        )
        .await;
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);

    let user = server.create_user("bob", Role::User).await;
    let mut api = server.api_client(&user).await;
    let image = noisy_png(300, 300);
    assert!(image.len() > hardening::FORM_BODY_LIMIT * 2);
    let response = upload(&mut api, "", image).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
}

#[tokio::test(start_paused = true)]
async fn slow_requests_time_out() {
    async fn slow() -> &'static str {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        "done"
    }
    let app = Router::new()
        .route("/slow", get(slow))
        .layer(axum::middleware::from_fn(hardening::timeout));
    let started = tokio::time::Instant::now();
    let response = app
        .oneshot(Request::get("/slow").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(started.elapsed(), REQUEST_TIMEOUT);
}

#[tokio::test]
async fn panics_become_500() {
    async fn boom() -> &'static str {
        panic!("handler bug");
    }
    let app = Router::new()
        .route("/boom", get(boom))
        .layer(axum::middleware::from_fn(hardening::catch_panic));
    let response = app
        .oneshot(Request::get("/boom").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn malformed_input_never_causes_server_errors() {
    let server = TestServer::new().await;
    let user = server.create_user("alice", Role::User).await;
    let mut browser = server.client("192.0.2.1");
    browser
        .login("alice", PASSWORD)
        .await
        .assert_redirect("/app");
    let mut api = server.api_client(&user).await;
    let long = format!("/{}", "a".repeat(8000));

    for path in [
        "/%00",
        "/..%2f..%2fetc%2fpasswd",
        "/static/..%2fCargo.toml",
        "/app/shots/%ff%fe",
        "/app?filter=%ff&page=-1",
        "/app?page=99999999999999999999",
        "/invite/%zz",
        long.as_str(),
    ] {
        let status = browser.get(path).await.status;
        assert!(
            status.is_client_error() || status.is_success(),
            "{path}: {status}"
        );
    }
    for path in [
        "/api/v1/screenshots?limit=abc",
        "/api/v1/screenshots?limit=%ff",
        "/api/v1/screenshots?limit=1&limit=2",
        "/api/v1/screenshots/..",
    ] {
        let status = api.get(path).await.status;
        assert!(
            status.is_client_error() || status.is_success(),
            "{path}: {status}"
        );
    }
    // Bodies that are not what the handler expects.
    for (path, content_type, body) in [
        (
            "/login",
            Some("application/x-www-form-urlencoded"),
            b"%%%".to_vec(),
        ),
        ("/login", Some("application/json"), b"{}".to_vec()),
        ("/login", None, b"csrf=x".to_vec()),
        (
            "/login/passkey/start",
            Some("application/json"),
            b"{not json".to_vec(),
        ),
        (
            "/login/passkey/finish",
            Some("application/json"),
            br#"{"ceremony":1}"#.to_vec(),
        ),
        (
            "/app/passkeys/register/finish",
            Some("application/json"),
            b"[]".to_vec(),
        ),
        (
            "/app/settings",
            Some("application/x-www-form-urlencoded"),
            vec![0xff, 0xfe],
        ),
        (
            "/api/v1/screenshots?visibility=public&expires_in_days=1e9",
            Some("image/png"),
            b"\x89PNG".to_vec(),
        ),
    ] {
        let status = if path.starts_with("/api") {
            api.send_raw("POST", path, content_type, body).await.status
        } else {
            browser
                .send_raw("POST", path, content_type, body)
                .await
                .status
        };
        assert!(status.is_client_error(), "{path}: {status}");
    }
    // Absurd headers.
    let mut weird = server.client("192.0.2.3");
    weird.headers.push((
        "authorization".into(),
        format!("Bearer {}", "x".repeat(10_000)),
    ));
    weird
        .headers
        .push(("cookie".into(), "scrn_session=;;;=;scrn_session=zz".into()));
    weird
        .headers
        .push(("accept-language".into(), "q=;;,,*;q=abc".into()));
    weird
        .headers
        .push(("x-forwarded-for".into(), "not-an-ip, , 999.1.1.1".into()));
    assert_eq!(
        weird.get("/api/v1/me").await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(weird.get("/login").await.status, StatusCode::OK);
    assert_eq!(weird.get("/app").await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn bad_tokens_are_rate_limited_per_ip() {
    let server = TestServer::new().await;
    let user = server.create_user("alice", Role::User).await;
    let mut attacker = server.client("203.0.113.66");
    attacker.headers.push((
        "authorization".into(),
        format!("Bearer scrn_{}", "a".repeat(43)),
    ));
    for _ in 0..API_FAILURES_PER_IP {
        assert_eq!(
            attacker.get("/api/v1/me").await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let response = attacker.get("/api/v1/me").await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers.contains_key("retry-after"));
    assert!(response.body.contains("too_many_requests"));

    // Other clients are not affected, and the window resets.
    let mut api = server.api_client(&user).await;
    assert_eq!(api.get("/api/v1/me").await.status, StatusCode::OK);
    server.clock.advance(scrn_server::API_FAILURE_WINDOW);
    assert_eq!(
        attacker.get("/api/v1/me").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn bad_tokens_behind_a_trusted_proxy_are_counted_per_client() {
    let server = TestServer::with_env(&[("SCRN_TRUSTED_PROXIES", "172.18.0.0/16")]).await;
    let mut first = server.client("172.18.0.2");
    first
        .headers
        .push(("x-forwarded-for".into(), "198.51.100.1".into()));
    first
        .headers
        .push(("authorization".into(), "Bearer bad".into()));
    for _ in 0..API_FAILURES_PER_IP {
        first.get("/api/v1/me").await;
    }
    assert_eq!(
        first.get("/api/v1/me").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );

    let mut second = server.client("172.18.0.2");
    second
        .headers
        .push(("x-forwarded-for".into(), "198.51.100.2".into()));
    second
        .headers
        .push(("authorization".into(), "Bearer bad".into()));
    assert_eq!(
        second.get("/api/v1/me").await.status,
        StatusCode::UNAUTHORIZED
    );

    // Without a trusted proxy the header is ignored and all share the peer.
    let server = TestServer::new().await;
    let mut spoofing = server.client("203.0.113.9");
    spoofing
        .headers
        .push(("authorization".into(), "Bearer bad".into()));
    for i in 0..=API_FAILURES_PER_IP {
        spoofing.headers.retain(|(n, _)| n != "x-forwarded-for");
        spoofing
            .headers
            .push(("x-forwarded-for".into(), format!("198.51.100.{i}")));
        spoofing.get("/api/v1/me").await;
    }
    assert_eq!(
        spoofing.get("/api/v1/me").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn uploads_are_rate_limited_per_user() {
    let server = TestServer::new().await;
    let user = server.create_user("alice", Role::User).await;
    let mut api = server.api_client(&user).await;
    let image = common::png_image(2, 2);
    for _ in 0..UPLOADS_PER_USER {
        assert_eq!(
            upload(&mut api, "", image.clone()).await.status,
            StatusCode::CREATED
        );
    }
    let response = upload(&mut api, "", image.clone()).await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);

    let other = server.create_user("bob", Role::User).await;
    let mut bob = server.api_client(&other).await;
    assert_eq!(
        upload(&mut bob, "", image).await.status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn registration_attempts_share_the_login_budget() {
    let server = TestServer::new().await;
    let token = server.state.issue_setup_token().unwrap();
    let mut browser = server.client("192.0.2.1");
    let csrf = browser.get("/setup").await.csrf();
    for _ in 0..LOGIN_ATTEMPTS_PER_IP {
        let response = browser
            .post(
                "/setup",
                &[
                    ("csrf", &csrf),
                    ("setup_token", "wrong"),
                    ("username", "admin"),
                    ("password", PASSWORD),
                    ("password_confirm", PASSWORD),
                ],
            )
            .await;
        assert_eq!(response.status, StatusCode::FORBIDDEN);
    }
    let response = browser
        .post(
            "/setup",
            &[
                ("csrf", &csrf),
                ("setup_token", &token),
                ("username", "admin"),
                ("password", PASSWORD),
                ("password_confirm", PASSWORD),
            ],
        )
        .await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers.contains_key("retry-after"));
}
