//! Integration tests for public short links.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{Client, TestResponse, TestServer, png_image, upload};
use scrn_proto::ScreenshotInfo;
use scrn_server::repo::screenshots::{self, PublicLink};
use scrn_server::repo::users::{Role, User};
use scrn_server::routes::{MAX_CACHE_SECONDS, PUBLIC_REQUESTS_PER_IP};
use scrn_server::storage;
use uuid::Uuid;

const DAY: i64 = 86_400;

async fn setup() -> (TestServer, User, Client) {
    let server = TestServer::new().await;
    let user = server.create_user("alice", Role::User).await;
    let api = server.api_client(&user).await;
    (server, user, api)
}

async fn upload_info(api: &mut Client, query: &str) -> ScreenshotInfo {
    let response = upload(api, query, png_image(32, 16)).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    serde_json::from_str(&response.body).unwrap()
}

fn short_path(info: &ScreenshotInfo) -> String {
    info.short_url
        .as_ref()
        .unwrap()
        .strip_prefix("https://scrn.test")
        .unwrap()
        .to_owned()
}

/// The response for an ID that never existed, for comparison.
async fn unknown(server: &TestServer) -> TestResponse {
    let response = server.client("203.0.113.50").get("/Zz9Zz9Zz").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    response
}

fn assert_same_404(response: &TestResponse, reference: &TestResponse) {
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.body, reference.body);
    assert_eq!(
        response.headers.get("content-type"),
        reference.headers.get("content-type")
    );
}

#[tokio::test]
async fn public_image_is_served_with_safe_headers() {
    let (server, user, mut api) = setup().await;
    let info = upload_info(&mut api, "?visibility=public").await;
    let path = short_path(&info);
    let id = Uuid::parse_str(&info.id).unwrap();
    let stored = std::fs::read(storage::image_path(
        &server.state.config.files_dir(),
        user.id,
        id,
    ))
    .unwrap();

    let mut visitor = server.client("203.0.113.7");
    for url in [path.clone(), format!("{path}.png")] {
        let response = visitor.get(&url).await;
        assert_eq!(response.status, StatusCode::OK, "{url}");
        let h = &response.headers;
        assert_eq!(h["content-type"], "image/png");
        assert_eq!(h["x-content-type-options"], "nosniff");
        assert!(
            h["content-security-policy"]
                .to_str()
                .unwrap()
                .starts_with("default-src 'none'")
        );
        assert_eq!(
            h["content-disposition"],
            format!("inline; filename=\"{}.png\"", &path[1..])
        );
        assert_eq!(
            h["cache-control"],
            format!("public, max-age={MAX_CACHE_SECONDS}")
        );
        assert!(h.contains_key("etag"));
        assert!(h["x-robots-tag"].to_str().unwrap().contains("noindex"));
        // The body is the re-encoded file.
        assert_eq!(response.bytes, stored);
    }

    // HEAD gives the headers without the body.
    let response = visitor.send_raw("HEAD", &path, None, Vec::new()).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers["content-type"], "image/png");
    assert!(response.bytes.is_empty());
}

#[tokio::test]
async fn etag_allows_revalidation() {
    let (server, _, mut api) = setup().await;
    let path = short_path(&upload_info(&mut api, "?visibility=public").await);
    let mut visitor = server.client("203.0.113.7");
    let etag = visitor.get(&path).await.headers["etag"]
        .to_str()
        .unwrap()
        .to_owned();
    visitor.headers.push(("if-none-match".into(), etag));
    let response = visitor.get(&path).await;
    assert_eq!(response.status, StatusCode::NOT_MODIFIED);
    assert!(response.body.is_empty());
}

#[tokio::test]
async fn private_images_are_never_served() {
    let (server, user, mut api) = setup().await;
    let reference = unknown(&server).await;
    let private = upload_info(&mut api, "").await;
    assert!(private.short_url.is_none());

    // Making a public image private kills its link.
    let public = upload_info(&mut api, "?visibility=public").await;
    let path = short_path(&public);
    let id = Uuid::parse_str(&public.id).unwrap();
    assert_eq!(
        server.client("203.0.113.7").get(&path).await.status,
        StatusCode::OK
    );
    screenshots::set_private(&server.state.db, id, user.id)
        .await
        .unwrap();
    let mut visitor = server.client("203.0.113.8");
    assert_same_404(&visitor.get(&path).await, &reference);
    assert_same_404(&visitor.get(&format!("{path}.png")).await, &reference);

    // The UUID is not a public address either.
    assert_same_404(&visitor.get(&format!("/{}", private.id)).await, &reference);
    assert_same_404(
        &visitor.get(&format!("/{}.png", private.id)).await,
        &reference,
    );
}

#[tokio::test]
async fn expired_links_return_the_same_404() {
    let (server, _, mut api) = setup().await;
    let reference = unknown(&server).await;
    let path = short_path(&upload_info(&mut api, "?visibility=public&expires_in_days=1").await);
    let mut visitor = server.client("203.0.113.7");

    // Shortly before the expiry, caching never outlives the link.
    server.clock.advance(DAY - 100);
    let response = visitor.get(&path).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers["cache-control"], "public, max-age=100");

    server.clock.advance(100);
    assert_same_404(&visitor.get(&path).await, &reference);
}

#[tokio::test]
async fn fixed_routes_win_and_reserved_names_are_never_served() {
    let (server, user, mut api) = setup().await;
    let mut visitor = server.client("203.0.113.7");
    assert_eq!(visitor.get("/healthz").await.status, StatusCode::OK);
    assert_eq!(visitor.get("/login").await.status, StatusCode::OK);
    visitor.get("/app").await.assert_redirect("/login");
    visitor.get("/admin").await.assert_redirect("/login");
    assert_eq!(
        visitor.get("/api/v1/me").await.status,
        StatusCode::UNAUTHORIZED
    );

    // Even a row with a reserved short ID (which the generator never
    // produces) is not reachable.
    let reference = unknown(&server).await;
    let info = upload_info(&mut api, "?visibility=public").await;
    let id = Uuid::parse_str(&info.id).unwrap();
    for reserved in ["admin123", "APIabcde", "static00"] {
        let link = PublicLink {
            short_id: reserved.to_owned(),
            expires_at: None,
        };
        assert!(
            screenshots::set_public(&server.state.db, id, user.id, &link)
                .await
                .unwrap()
        );
        assert_same_404(&visitor.get(&format!("/{reserved}")).await, &reference);
    }
    for name in ["/api", "/app.png", "/abc", "/abcdefghi", "/abc-efgh"] {
        assert_same_404(&visitor.get(name).await, &reference);
    }
}

#[tokio::test]
async fn missing_file_gives_404() {
    let (server, user, mut api) = setup().await;
    let reference = unknown(&server).await;
    let info = upload_info(&mut api, "?visibility=public").await;
    let id = Uuid::parse_str(&info.id).unwrap();
    std::fs::remove_file(storage::image_path(
        &server.state.config.files_dir(),
        user.id,
        id,
    ))
    .unwrap();
    let response = server.client("203.0.113.7").get(&short_path(&info)).await;
    assert_same_404(&response, &reference);
}

#[tokio::test]
async fn public_links_are_rate_limited_per_ip() {
    let (server, _, mut api) = setup().await;
    let path = short_path(&upload_info(&mut api, "?visibility=public").await);
    let mut visitor = server.client("203.0.113.7");
    for _ in 0..PUBLIC_REQUESTS_PER_IP {
        assert_eq!(visitor.get(&path).await.status, StatusCode::OK);
    }
    let response = visitor.get(&path).await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers["retry-after"], "60");
    // Unknown IDs count too, so guessing is limited as well.
    assert_eq!(
        visitor.get("/Zz9Zz9Zz").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );

    // A spoofed X-Forwarded-For from an untrusted peer does not help.
    visitor
        .headers
        .push(("x-forwarded-for".into(), "192.0.2.99".into()));
    assert_eq!(
        visitor.get(&path).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );

    // Other clients are not affected, and the window resets.
    assert_eq!(
        server.client("203.0.113.8").get(&path).await.status,
        StatusCode::OK
    );
    server.clock.advance(60);
    let mut visitor = server.client("203.0.113.7");
    assert_eq!(visitor.get(&path).await.status, StatusCode::OK);
}

#[tokio::test]
async fn clients_behind_a_trusted_proxy_are_limited_separately() {
    let server = TestServer::with_env(&[("SCRN_TRUSTED_PROXIES", "172.18.0.0/16")]).await;
    let user = server.create_user("alice", Role::User).await;
    let mut api = server.api_client(&user).await;
    let path = short_path(&upload_info(&mut api, "?visibility=public").await);

    let mut first = server.client("172.18.0.2");
    first
        .headers
        .push(("x-forwarded-for".into(), "198.51.100.10".into()));
    for _ in 0..PUBLIC_REQUESTS_PER_IP {
        first.get(&path).await;
    }
    assert_eq!(first.get(&path).await.status, StatusCode::TOO_MANY_REQUESTS);

    let mut second = server.client("172.18.0.2");
    second
        .headers
        .push(("x-forwarded-for".into(), "198.51.100.11".into()));
    assert_eq!(second.get(&path).await.status, StatusCode::OK);
}
