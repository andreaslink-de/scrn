//! Integration tests for changing passwords in the settings and for the
//! single-use password reset links created by an admin.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{Client, PASSWORD, TestResponse, TestServer};
use scrn_server::auth::RESET_LINK_TTL;
use scrn_server::clock::Clock;
use scrn_server::repo::users::Role;

const NEW_PASSWORD: &str = "a brand new passphrase";

async fn change(browser: &mut Client, current: &str, new: &str, confirm: &str) -> TestResponse {
    browser
        .submit(
            "/app/settings",
            "/app/settings/password",
            &[
                ("current_password", current),
                ("password", new),
                ("password_confirm", confirm),
            ],
        )
        .await
}

/// The path of a reset link shown on the admin page.
fn reset_path(page: &TestResponse) -> String {
    let marker = r#"id="reset-link" type="text" readonly value=""#;
    let start = page.body.find(marker).expect("page shows a reset link") + marker.len();
    let end = page.body[start..].find('"').unwrap();
    let link = &page.body[start..start + end];
    link.strip_prefix("https://scrn.test").unwrap().to_owned()
}

async fn admin_creates_link(admin: &mut Client, user_id: i64) -> String {
    let page = admin
        .submit("/admin", &format!("/admin/users/{user_id}/reset"), &[])
        .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(page.body.contains("bob"));
    reset_path(&page)
}

async fn set_via_link(browser: &mut Client, path: &str, password: &str) -> TestResponse {
    browser
        .submit(
            path,
            path,
            &[("password", password), ("password_confirm", password)],
        )
        .await
}

#[tokio::test]
async fn change_password_with_the_current_one() {
    let server = TestServer::new().await;
    server.create_user("bob", Role::User).await;
    let mut browser = server.client("192.0.2.1");
    browser.login("bob", PASSWORD).await.assert_redirect("/app");
    let mut other_device = server.client("192.0.2.2");
    other_device
        .login("bob", PASSWORD)
        .await
        .assert_redirect("/app");

    let page = browser.get("/app/settings").await;
    assert!(page.body.contains(r#"action="/app/settings/password""#));
    assert!(page.body.contains(r#"name="current_password""#));
    assert!(
        page.body
            .contains(r#"value="bob" autocomplete="username" readonly"#),
        "password managers see which account changes"
    );

    // Wrong current password, mismatched or too short new ones.
    let wrong = change(&mut browser, "not my password", NEW_PASSWORD, NEW_PASSWORD).await;
    assert_eq!(wrong.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(wrong.body.contains("Das aktuelle Passwort ist falsch."));
    let mismatch = change(&mut browser, PASSWORD, NEW_PASSWORD, "something else").await;
    assert_eq!(mismatch.status, StatusCode::UNPROCESSABLE_ENTITY);
    let short = change(&mut browser, PASSWORD, "short", "short").await;
    assert_eq!(short.status, StatusCode::UNPROCESSABLE_ENTITY);

    change(&mut browser, PASSWORD, NEW_PASSWORD, NEW_PASSWORD)
        .await
        .assert_redirect("/app/settings?password=changed#password");
    let page = browser.get("/app/settings?password=changed").await;
    assert!(page.body.contains("Passwort geändert."));

    // This session stays, the other device is signed out.
    assert_eq!(browser.get("/app").await.status, StatusCode::OK);
    other_device.get("/app").await.assert_redirect("/login");

    let mut fresh = server.client("192.0.2.3");
    assert_eq!(
        fresh.login("bob", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    fresh
        .login("bob", NEW_PASSWORD)
        .await
        .assert_redirect("/app");
}

#[tokio::test]
async fn guessing_the_current_password_is_rate_limited() {
    let server = TestServer::new().await;
    server.create_user("bob", Role::User).await;
    let mut browser = server.client("192.0.2.1");
    browser.login("bob", PASSWORD).await.assert_redirect("/app");
    for _ in 0..scrn_server::LOGIN_FAILURES_PER_USER {
        let response = change(&mut browser, "guess", NEW_PASSWORD, NEW_PASSWORD).await;
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let blocked = change(&mut browser, PASSWORD, NEW_PASSWORD, NEW_PASSWORD).await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn password_change_needs_a_session_and_csrf() {
    let server = TestServer::new().await;
    server.create_user("bob", Role::User).await;
    let mut visitor = server.client("192.0.2.1");
    visitor
        .post(
            "/app/settings/password",
            &[
                ("csrf", "x"),
                ("current_password", PASSWORD),
                ("password", NEW_PASSWORD),
                ("password_confirm", NEW_PASSWORD),
            ],
        )
        .await
        .assert_redirect("/login");

    let mut browser = server.client("192.0.2.1");
    browser.login("bob", PASSWORD).await.assert_redirect("/app");
    let forged = browser
        .post(
            "/app/settings/password",
            &[
                ("csrf", "forged"),
                ("current_password", PASSWORD),
                ("password", NEW_PASSWORD),
                ("password_confirm", NEW_PASSWORD),
            ],
        )
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_reset_link_sets_a_new_password_once() {
    let server = TestServer::new().await;
    server.create_user("admin", Role::Admin).await;
    let bob = server.create_user("bob", Role::User).await;
    let mut admin = server.client("192.0.2.1");
    admin.login("admin", PASSWORD).await.assert_redirect("/app");
    let mut bob_device = server.client("192.0.2.2");
    bob_device
        .login("bob", PASSWORD)
        .await
        .assert_redirect("/app");

    let page = admin.get("/admin").await;
    assert!(
        page.body
            .contains(&format!(r#"action="/admin/users/{}/reset""#, bob.id))
    );
    let path = admin_creates_link(&mut admin, bob.id).await;
    assert!(path.starts_with("/reset/"));

    // The page names the account, for the password manager too.
    let mut browser = server.client("192.0.2.3");
    let form = browser.get(&path).await;
    assert_eq!(form.status, StatusCode::OK);
    assert!(
        form.body
            .contains(r#"value="bob" autocomplete="username" readonly"#)
    );

    let short = set_via_link(&mut browser, &path, "short").await;
    assert_eq!(short.status, StatusCode::UNPROCESSABLE_ENTITY);
    set_via_link(&mut browser, &path, NEW_PASSWORD)
        .await
        .assert_redirect("/login?reset=1");
    let login = browser.get("/login?reset=1").await;
    assert!(login.body.contains("Dein neues Passwort ist gespeichert."));

    // All of Bob's sessions ended; only the new password works.
    bob_device.get("/app").await.assert_redirect("/login");
    assert_eq!(
        browser.login("bob", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    browser
        .login("bob", NEW_PASSWORD)
        .await
        .assert_redirect("/app");

    // The link is used up.
    let mut again = server.client("192.0.2.4");
    assert_eq!(again.get(&path).await.status, StatusCode::NOT_FOUND);
    let csrf = again.get("/login").await.csrf();
    let reuse = again
        .post(
            &path,
            &[
                ("csrf", &csrf),
                ("password", "yet another passphrase"),
                ("password_confirm", "yet another passphrase"),
            ],
        )
        .await;
    assert_eq!(reuse.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reset_links_expire_and_only_the_newest_works() {
    let server = TestServer::new().await;
    server.create_user("admin", Role::Admin).await;
    let bob = server.create_user("bob", Role::User).await;
    let mut admin = server.client("192.0.2.1");
    admin.login("admin", PASSWORD).await.assert_redirect("/app");

    let first = admin_creates_link(&mut admin, bob.id).await;
    let second = admin_creates_link(&mut admin, bob.id).await;
    assert_ne!(first, second);
    let mut browser = server.client("192.0.2.3");
    assert_eq!(browser.get(&first).await.status, StatusCode::NOT_FOUND);
    assert_eq!(browser.get(&second).await.status, StatusCode::OK);

    server.clock.advance(RESET_LINK_TTL);
    assert_eq!(browser.get(&second).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        browser.get("/reset/not-a-token").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn only_admins_create_reset_links() {
    let server = TestServer::new().await;
    let admin = server.create_user("admin", Role::Admin).await;
    server.create_user("bob", Role::User).await;
    let mut bob = server.client("192.0.2.2");
    bob.login("bob", PASSWORD).await.assert_redirect("/app");
    let response = bob
        .submit(
            "/app/settings",
            &format!("/admin/users/{}/reset", admin.id),
            &[],
        )
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);

    let mut admin_browser = server.client("192.0.2.1");
    admin_browser
        .login("admin", PASSWORD)
        .await
        .assert_redirect("/app");
    let missing = admin_browser
        .submit("/admin", "/admin/users/9999/reset", &[])
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn command_line_reset_link_recovers_the_admin() {
    let server = TestServer::new().await;
    server.create_user("admin", Role::Admin).await;
    let link = scrn_server::issue_cli_reset_link(
        &server.state.db,
        &server.state.config,
        "ADMIN",
        server.clock.now(),
    )
    .await
    .unwrap()
    .expect("user exists");
    let path = link.strip_prefix("https://scrn.test").unwrap();
    let mut browser = server.client("192.0.2.1");
    set_via_link(&mut browser, path, NEW_PASSWORD)
        .await
        .assert_redirect("/login?reset=1");
    browser
        .login("admin", NEW_PASSWORD)
        .await
        .assert_redirect("/app");

    assert_eq!(
        scrn_server::issue_cli_reset_link(
            &server.state.db,
            &server.state.config,
            "nobody",
            server.clock.now(),
        )
        .await
        .unwrap(),
        None
    );
}

#[tokio::test]
async fn setup_token_field_is_not_a_password_field() {
    let server = TestServer::new().await;
    scrn_server::announce_setup_token(&server.state)
        .await
        .unwrap();
    let page = server.client("192.0.2.1").get("/setup").await;
    assert!(
        page.body
            .contains(r#"name="setup_token" type="text" autocomplete="one-time-code""#)
    );
}
