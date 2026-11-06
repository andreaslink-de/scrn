//! Integration tests for API tokens: the management page and bearer
//! authentication of `/api/v1`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{Client, PASSWORD, START, TestResponse, TestServer};
use scrn_proto::{ApiErrorBody, MeResponse};
use scrn_server::api::TOKEN_TOUCH_INTERVAL;
use scrn_server::crypto;
use scrn_server::repo::api_tokens;
use scrn_server::repo::users::{self, Role, User};

/// Extracts the one-time token from the page shown after creating it.
fn shown_token(body: &str) -> String {
    let start = body.find(r#"value="scrn_"#).expect("page shows the token") + r#"value=""#.len();
    let end = body[start..].find('"').unwrap();
    body[start..start + end].to_owned()
}

async fn logged_in(server: &TestServer, name: &str, role: Role) -> (User, Client) {
    let user = server.create_user(name, role).await;
    let mut browser = server.client("192.0.2.1");
    browser.login(name, PASSWORD).await.assert_redirect("/app");
    (user, browser)
}

async fn create_token(browser: &mut Client, name: &str) -> String {
    let response = browser
        .submit("/app/settings", "/app/tokens", &[("name", name)])
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    shown_token(&response.body)
}

fn api_client(server: &TestServer, token: &str) -> Client {
    let mut client = server.client("198.51.100.1");
    client
        .headers
        .push(("authorization".into(), format!("Bearer {token}")));
    client
}

fn assert_unauthorized(response: &TestResponse) {
    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.body
    );
    assert_eq!(response.headers["www-authenticate"], "Bearer");
    assert_eq!(response.headers["cache-control"], "no-store");
    let body: ApiErrorBody = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body.error, "unauthorized");
}

#[tokio::test]
async fn token_is_shown_once_and_authenticates_me() {
    let server = TestServer::new().await;
    let (_, mut browser) = logged_in(&server, "alice", Role::User).await;

    let page = browser.get("/app/settings").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Noch keine Tokens"));

    let token = create_token(&mut browser, "  Laptop ").await;
    assert!(token.starts_with("scrn_"));
    assert_eq!(token.len(), 48);

    // The list shows the name but never the token again.
    let page = browser.get("/app/settings").await;
    assert!(page.body.contains("<td>Laptop</td>"));
    assert!(page.body.contains("<td>nie</td>"));
    assert!(!page.body.contains(&token));

    let response = api_client(&server, &token).get("/api/v1/me").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(response.headers["content-type"], "application/json");
    assert_eq!(response.headers["cache-control"], "no-store");
    let me: MeResponse = serde_json::from_str(&response.body).unwrap();
    assert_eq!(me.username, "alice");
    assert_eq!(me.server_version, env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn only_the_hash_is_stored() {
    let server = TestServer::new().await;
    let (user, mut browser) = logged_in(&server, "alice", Role::User).await;
    let token = create_token(&mut browser, "laptop").await;

    let row: (Vec<u8>,) = sqlx::query_as("SELECT token_hash FROM api_tokens WHERE user_id = ?")
        .bind(user.id)
        .fetch_one(&server.state.db)
        .await
        .unwrap();
    assert_eq!(row.0, crypto::sha256(token.as_bytes()));

    // Neither the database file nor its write-ahead log contain the token.
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&server.state.db)
        .await
        .unwrap();
    let db_path = server.state.config.database_path();
    for path in [db_path.clone(), db_path.with_extension("db-wal")] {
        if let Ok(bytes) = std::fs::read(&path) {
            let digits = &token.as_bytes()["scrn_".len()..];
            assert!(
                !bytes.windows(digits.len()).any(|w| w == digits),
                "{path:?} contains the token"
            );
        }
    }
}

#[tokio::test]
async fn invalid_tokens_are_rejected() {
    let server = TestServer::new().await;
    let (_, mut browser) = logged_in(&server, "alice", Role::User).await;
    let token = create_token(&mut browser, "laptop").await;

    let well_formed_unknown = format!("scrn_{}", "a".repeat(43));
    for bad in [
        "",
        "garbage",
        well_formed_unknown.as_str(),
        &token[..token.len() - 1],
        &token.to_uppercase(),
    ] {
        assert_unauthorized(&api_client(&server, bad).get("/api/v1/me").await);
    }

    // No header, a different scheme, and a session cookie are not enough.
    assert_unauthorized(&server.client("198.51.100.1").get("/api/v1/me").await);
    let mut basic = server.client("198.51.100.1");
    basic
        .headers
        .push(("authorization".into(), format!("Basic {token}")));
    assert_unauthorized(&basic.get("/api/v1/me").await);
    assert_unauthorized(&browser.get("/api/v1/me").await);
}

#[tokio::test]
async fn revoked_token_is_rejected() {
    let server = TestServer::new().await;
    let (user, mut browser) = logged_in(&server, "alice", Role::User).await;
    let token = create_token(&mut browser, "laptop").await;
    let mut api = api_client(&server, &token);
    assert_eq!(api.get("/api/v1/me").await.status, StatusCode::OK);

    let id = api_tokens::list_for_user(&server.state.db, user.id)
        .await
        .unwrap()[0]
        .id;
    browser
        .submit("/app/settings", &format!("/app/tokens/{id}/revoke"), &[])
        .await
        .assert_redirect("/app/settings#tokens");

    assert_unauthorized(&api.get("/api/v1/me").await);
    let page = browser.get("/app/settings").await;
    assert!(page.body.contains("<td>Widerrufen</td>"));
}

#[tokio::test]
async fn token_of_disabled_user_is_rejected() {
    let server = TestServer::new().await;
    let (user, mut browser) = logged_in(&server, "alice", Role::User).await;
    let token = create_token(&mut browser, "laptop").await;
    let mut api = api_client(&server, &token);

    users::set_disabled(&server.state.db, user.id, Some(START))
        .await
        .unwrap();
    assert_unauthorized(&api.get("/api/v1/me").await);

    // Re-enabling the account makes the token work again.
    users::set_disabled(&server.state.db, user.id, None)
        .await
        .unwrap();
    assert_eq!(api.get("/api/v1/me").await.status, StatusCode::OK);
}

#[tokio::test]
async fn last_used_is_updated_throttled() {
    let server = TestServer::new().await;
    let (user, mut browser) = logged_in(&server, "alice", Role::User).await;
    let token = create_token(&mut browser, "laptop").await;
    let mut api = api_client(&server, &token);
    let last_used = || async {
        api_tokens::list_for_user(&server.state.db, user.id)
            .await
            .unwrap()[0]
            .last_used_at
    };
    assert_eq!(last_used().await, None);

    api.get("/api/v1/me").await;
    assert_eq!(last_used().await, Some(START));

    server.clock.advance(TOKEN_TOUCH_INTERVAL - 1);
    api.get("/api/v1/me").await;
    assert_eq!(last_used().await, Some(START));

    server.clock.advance(1);
    api.get("/api/v1/me").await;
    assert_eq!(last_used().await, Some(START + TOKEN_TOUCH_INTERVAL));

    let page = browser.get("/app/settings").await;
    assert!(!page.body.contains("<td>nie</td>"));
}

#[tokio::test]
async fn unknown_api_paths_need_a_token_and_return_json() {
    let server = TestServer::new().await;
    let (_, mut browser) = logged_in(&server, "alice", Role::User).await;
    let token = create_token(&mut browser, "laptop").await;

    assert_unauthorized(&server.client("198.51.100.1").get("/api/v1/nope").await);
    let response = api_client(&server, &token).get("/api/v1/nope").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    let body: ApiErrorBody = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body.error, "not_found");
}

#[tokio::test]
async fn users_cannot_revoke_tokens_of_others() {
    let server = TestServer::new().await;
    let (alice, mut alice_browser) = logged_in(&server, "alice", Role::User).await;
    let token = create_token(&mut alice_browser, "laptop").await;
    let (_, mut bob_browser) = logged_in(&server, "bob", Role::Admin).await;

    let id = api_tokens::list_for_user(&server.state.db, alice.id)
        .await
        .unwrap()[0]
        .id;
    bob_browser
        .submit("/app/settings", &format!("/app/tokens/{id}/revoke"), &[])
        .await
        .assert_redirect("/app/settings#tokens");

    assert_eq!(
        api_client(&server, &token).get("/api/v1/me").await.status,
        StatusCode::OK
    );
    // Bob does not see Alice's tokens either.
    assert!(
        !bob_browser
            .get("/app/settings")
            .await
            .body
            .contains("laptop")
    );
}

#[tokio::test]
async fn token_forms_need_csrf_and_login() {
    let server = TestServer::new().await;
    server
        .client("192.0.2.9")
        .get("/app/settings")
        .await
        .assert_redirect("/login");

    let (user, mut browser) = logged_in(&server, "alice", Role::User).await;
    let response = browser
        .post("/app/tokens", &[("csrf", "wrong"), ("name", "laptop")])
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert!(
        api_tokens::list_for_user(&server.state.db, user.id)
            .await
            .unwrap()
            .is_empty()
    );

    create_token(&mut browser, "laptop").await;
    let id = api_tokens::list_for_user(&server.state.db, user.id)
        .await
        .unwrap()[0]
        .id;
    let response = browser
        .post(&format!("/app/tokens/{id}/revoke"), &[("csrf", "wrong")])
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert!(
        api_tokens::list_for_user(&server.state.db, user.id)
            .await
            .unwrap()[0]
            .revoked_at
            .is_none()
    );
}

#[tokio::test]
async fn invalid_token_names_are_rejected() {
    let server = TestServer::new().await;
    let (user, mut browser) = logged_in(&server, "alice", Role::User).await;
    let long = "x".repeat(101);
    for name in ["", "   ", long.as_str()] {
        let response = browser
            .submit("/app/settings", "/app/tokens", &[("name", name)])
            .await;
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(response.body.contains("1 bis 100 Zeichen"));
        assert!(!response.body.contains("value=\"scrn_"));
    }
    // Names are escaped in the list.
    create_token(&mut browser, "<b>x</b>").await;
    let page = browser.get("/app/settings").await;
    assert!(!page.body.contains("<b>x</b>"));
    assert!(page.body.contains("&#60;b&#62;x&#60;/b&#62;") || page.body.contains("&lt;b&gt;x"));
    assert_eq!(
        api_tokens::list_for_user(&server.state.db, user.id)
            .await
            .unwrap()
            .len(),
        1
    );
}
