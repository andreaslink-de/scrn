//! Integration tests for setup, login, invitations and user administration.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{PASSWORD, START, TestServer};
use scrn_server::repo::users::{self, Role};
use scrn_server::{LOGIN_ATTEMPTS_PER_IP, LOGIN_FAILURES_PER_USER, announce_setup_token};

fn invite_link(body: &str) -> String {
    let start = body
        .find("https://scrn.test/invite/")
        .expect("page shows the invite link");
    let end = body[start..].find('"').unwrap();
    body[start + "https://scrn.test".len()..start + end].to_owned()
}

// --- Setup ------------------------------------------------------------------

#[tokio::test]
async fn setup_flow_creates_admin_once() {
    let server = TestServer::new().await;
    let token = server.state.issue_setup_token().unwrap();
    let mut browser = server.client("192.0.2.1");

    browser.get("/").await.assert_redirect("/setup");
    let page = browser.get("/setup").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(r#"<html lang="de">"#));

    // Wrong setup token.
    let wrong = "0".repeat(64);
    let response = browser
        .submit(
            "/setup",
            "/setup",
            &[
                ("setup_token", &wrong),
                ("username", "admin"),
                ("password", PASSWORD),
                ("password_confirm", PASSWORD),
            ],
        )
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert!(response.body.contains("Einrichtungscode ist ungültig"));

    // Password too short.
    let response = browser
        .submit(
            "/setup",
            "/setup",
            &[
                ("setup_token", &token),
                ("username", "admin"),
                ("password", "short"),
                ("password_confirm", "short"),
            ],
        )
        .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(users::count(&server.state.db).await.unwrap(), 0);

    // Success: admin is created and logged in.
    let response = browser
        .submit(
            "/setup",
            "/setup",
            &[
                ("setup_token", &token),
                ("username", "admin"),
                ("password", PASSWORD),
                ("password_confirm", PASSWORD),
            ],
        )
        .await;
    response.assert_redirect("/admin");
    let admin = users::get_by_username(&server.state.db, "admin")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admin.role, Role::Admin);
    assert!(admin.password_hash.unwrap().starts_with("$argon2id$"));
    let page = browser.get("/admin").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("admin"));

    // Setup is locked now, for GET and POST, with or without the old token.
    let mut other = server.client("192.0.2.2");
    assert_eq!(other.get("/setup").await.status, StatusCode::NOT_FOUND);
    let csrf_page = other.get("/login").await;
    let response = other
        .post(
            "/setup",
            &[
                ("csrf", &csrf_page.csrf()),
                ("setup_token", &token),
                ("username", "evil"),
                ("password", PASSWORD),
                ("password_confirm", PASSWORD),
            ],
        )
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(!server.state.check_setup_token(&token));
    assert_eq!(users::count(&server.state.db).await.unwrap(), 1);
}

#[tokio::test]
async fn setup_token_is_only_issued_without_users() {
    let server = TestServer::new().await;
    announce_setup_token(&server.state).await.unwrap();
    assert!(server.state.setup_token.lock().unwrap().is_some());

    let server = TestServer::new().await;
    server.create_user("admin", Role::Admin).await;
    announce_setup_token(&server.state).await.unwrap();
    assert!(server.state.setup_token.lock().unwrap().is_none());
}

#[tokio::test]
async fn forms_without_valid_csrf_token_are_rejected() {
    let server = TestServer::new().await;
    let token = server.state.issue_setup_token().unwrap();
    let mut browser = server.client("192.0.2.1");
    let fields = [
        ("setup_token", token.as_str()),
        ("username", "admin"),
        ("password", PASSWORD),
        ("password_confirm", PASSWORD),
    ];

    // No cookie, no field.
    let mut with_csrf = vec![("csrf", "")];
    with_csrf.extend_from_slice(&fields);
    assert_eq!(
        browser.post("/setup", &with_csrf).await.status,
        StatusCode::FORBIDDEN
    );

    // Cookie present, but the field does not match it.
    browser.get("/setup").await;
    let forged = "f".repeat(64);
    let mut with_csrf = vec![("csrf", forged.as_str())];
    with_csrf.extend_from_slice(&fields);
    assert_eq!(
        browser.post("/setup", &with_csrf).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(users::count(&server.state.db).await.unwrap(), 0);
}

// --- Login ------------------------------------------------------------------

#[tokio::test]
async fn login_and_logout() {
    let server = TestServer::new().await;
    server.create_user("bob", Role::User).await;
    let mut browser = server.client("192.0.2.1");

    browser.get("/app").await.assert_redirect("/login");
    let response = browser.login("Bob", PASSWORD).await;
    response.assert_redirect("/app");
    let cookie = response
        .set_cookies
        .iter()
        .find(|c| c.starts_with("scrn_session="))
        .unwrap();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    assert!(!cookie.contains("Secure"));

    let page = browser.get("/app").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Angemeldet als bob."));
    browser.get("/login").await.assert_redirect("/app");

    // Logout needs the session CSRF token.
    let stolen_session = browser.cookies.clone();
    assert_eq!(
        browser.post("/logout", &[("csrf", "nope")]).await.status,
        StatusCode::FORBIDDEN
    );
    let csrf = page.csrf();
    browser
        .post("/logout", &[("csrf", &csrf)])
        .await
        .assert_redirect("/login");
    browser.get("/app").await.assert_redirect("/login");

    // The old session cookie is dead on the server, not just in the browser.
    let mut replay = server.client("192.0.2.1");
    replay.cookies = stolen_session;
    replay.get("/app").await.assert_redirect("/login");
}

#[tokio::test]
async fn secure_cookies_use_host_prefix() {
    let server = TestServer::with_env(&[("SCRN_COOKIE_SECURE", "true")]).await;
    server.create_user("bob", Role::User).await;
    let mut browser = server.client("192.0.2.1");
    let response = browser.login("bob", PASSWORD).await;
    response.assert_redirect("/app");
    let cookie = response
        .set_cookies
        .iter()
        .find(|c| c.starts_with("__Host-scrn_session="))
        .unwrap();
    assert!(cookie.contains("; Secure"));
    assert!(cookie.contains("HttpOnly"));
    assert_eq!(browser.get("/app").await.status, StatusCode::OK);
}

#[tokio::test]
async fn failed_logins_look_identical() {
    let server = TestServer::new().await;
    server.create_user("bob", Role::User).await;
    let carol = server.create_user("carol", Role::User).await;
    users::set_disabled(&server.state.db, carol.id, Some(START))
        .await
        .unwrap();

    let mut responses = Vec::new();
    for (name, password) in [
        ("bob", "wrong password!!"),
        ("nobody", PASSWORD),
        ("carol", PASSWORD),
        ("", PASSWORD),
    ] {
        let mut browser = server.client("192.0.2.1");
        responses.push(browser.login(name, password).await);
    }
    for response in &responses {
        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
        assert!(
            response
                .body
                .contains("Benutzername oder Passwort ist falsch.")
        );
    }
    // Same page apart from the per-browser CSRF token.
    let normalize = |r: &common::TestResponse| r.body.replace(&r.csrf(), "");
    let first = normalize(&responses[0]);
    assert!(responses.iter().all(|r| normalize(r) == first));
}

#[tokio::test]
async fn login_is_rate_limited_per_username() {
    let server = TestServer::new().await;
    server.create_user("bob", Role::User).await;
    for i in 0..LOGIN_FAILURES_PER_USER {
        // Different IPs, so only the per-username limit applies.
        let mut attacker = server.client(&format!("198.51.100.{i}"));
        assert_eq!(
            attacker.login("bob", "wrong password!!").await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let mut owner = server.client("192.0.2.1");
    let response = owner.login("BOB", PASSWORD).await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);

    // The same limit applies to unknown names, so it reveals nothing.
    for i in 0..LOGIN_FAILURES_PER_USER {
        let mut attacker = server.client(&format!("198.51.100.{i}"));
        attacker.login("ghost", "wrong password!!").await;
    }
    let mut attacker = server.client("198.51.100.200");
    assert_eq!(
        attacker.login("ghost", "wrong password!!").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );

    // After the window the owner can log in again.
    server.clock.advance(scrn_server::LOGIN_WINDOW);
    owner.login("bob", PASSWORD).await.assert_redirect("/app");
}

#[tokio::test]
async fn login_is_rate_limited_per_ip() {
    let server = TestServer::new().await;
    server.create_user("bob", Role::User).await;
    let mut attacker = server.client("198.51.100.1");
    for i in 0..LOGIN_ATTEMPTS_PER_IP {
        let response = attacker
            .login(&format!("user{i}"), "wrong password!!")
            .await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        attacker.login("bob", PASSWORD).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Others are not affected.
    let mut other = server.client("192.0.2.1");
    other.login("bob", PASSWORD).await.assert_redirect("/app");
}

#[tokio::test]
async fn spoofed_forwarded_for_does_not_bypass_ip_limit() {
    // No trusted proxies: X-Forwarded-For must be ignored.
    let server = TestServer::new().await;
    let mut attacker = server.client("198.51.100.1");
    for i in 0..LOGIN_ATTEMPTS_PER_IP {
        attacker.headers = vec![("x-forwarded-for".into(), format!("10.0.0.{i}"))];
        attacker
            .login(&format!("user{i}"), "wrong password!!")
            .await;
    }
    attacker.headers = vec![("x-forwarded-for".into(), "10.0.1.1".into())];
    assert_eq!(
        attacker.login("x", "wrong password!!").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn sessions_expire() {
    let server = TestServer::new().await;
    server.create_user("bob", Role::User).await;
    let mut browser = server.client("192.0.2.1");
    browser.login("bob", PASSWORD).await.assert_redirect("/app");
    server.clock.advance(scrn_server::auth::SESSION_TTL);
    browser.get("/app").await.assert_redirect("/login");
}

// --- Invitations ------------------------------------------------------------

async fn admin_with_invite(server: &TestServer) -> (common::Client, String) {
    server.create_user("admin", Role::Admin).await;
    let mut admin = server.client("192.0.2.1");
    admin.login("admin", PASSWORD).await.assert_redirect("/app");
    let response = admin
        .submit("/admin", "/admin/invites", &[("days", "7")])
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let link = invite_link(&response.body);
    (admin, link)
}

#[tokio::test]
async fn invitation_is_single_use() {
    let server = TestServer::new().await;
    let (_admin, link) = admin_with_invite(&server).await;

    // The token is not stored in clear text.
    let token = link.trim_start_matches("/invite/");
    let stored: Vec<Vec<u8>> = sqlx::query_scalar("SELECT token_hash FROM invites")
        .fetch_all(&server.state.db)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert!(
        !stored[0]
            .windows(token.len())
            .any(|w| w == token.as_bytes())
    );

    let mut newcomer = server.client("192.0.2.5");
    assert_eq!(newcomer.get(&link).await.status, StatusCode::OK);
    let response = newcomer
        .submit(
            &link,
            &link,
            &[
                ("username", "dave"),
                ("password", PASSWORD),
                ("password_confirm", PASSWORD),
            ],
        )
        .await;
    response.assert_redirect("/app");
    assert!(newcomer.get("/app").await.body.contains("dave"));
    let dave = users::get_by_username(&server.state.db, "dave")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dave.role, Role::User);

    // A second use of the same link fails, for GET and POST.
    let mut second = server.client("192.0.2.6");
    let page = second.get(&link).await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert!(page.body.contains("bereits benutzt"));
    let csrf = second.get("/login").await.csrf();
    let response = second
        .post(
            &link,
            &[
                ("csrf", &csrf),
                ("username", "eve"),
                ("password", PASSWORD),
                ("password_confirm", PASSWORD),
            ],
        )
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(
        users::get_by_username(&server.state.db, "eve")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn invitation_expires() {
    let server = TestServer::new().await;
    let (_admin, link) = admin_with_invite(&server).await;
    server.clock.advance(7 * 86_400);
    let mut newcomer = server.client("192.0.2.5");
    assert_eq!(newcomer.get(&link).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invalid_invitation_links_look_identical() {
    let server = TestServer::new().await;
    let mut browser = server.client("192.0.2.5");
    let unknown = browser.get(&format!("/invite/{}", "a".repeat(64))).await;
    let garbage = browser.get("/invite/not-a-token").await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.body, garbage.body);
}

#[tokio::test]
async fn revoked_invitation_cannot_be_used() {
    let server = TestServer::new().await;
    let (mut admin, link) = admin_with_invite(&server).await;
    let page = admin.get("/admin").await;
    let csrf = page.csrf();
    let invite_id: i64 = sqlx::query_scalar("SELECT id FROM invites")
        .fetch_one(&server.state.db)
        .await
        .unwrap();
    admin
        .post(
            &format!("/admin/invites/{invite_id}/delete"),
            &[("csrf", &csrf)],
        )
        .await
        .assert_redirect("/admin");
    let mut newcomer = server.client("192.0.2.5");
    assert_eq!(newcomer.get(&link).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn registration_rejects_taken_and_invalid_usernames() {
    let server = TestServer::new().await;
    let (_admin, link) = admin_with_invite(&server).await;
    let mut newcomer = server.client("192.0.2.5");
    for (name, status) in [
        ("ADMIN", StatusCode::CONFLICT),
        ("bad name", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let response = newcomer
            .submit(
                &link,
                &link,
                &[
                    ("username", name),
                    ("password", PASSWORD),
                    ("password_confirm", PASSWORD),
                ],
            )
            .await;
        assert_eq!(response.status, status, "{name}");
    }
    // The invite is still usable after the failed attempts.
    assert_eq!(newcomer.get(&link).await.status, StatusCode::OK);
}

#[tokio::test]
async fn invite_validity_is_checked() {
    let server = TestServer::new().await;
    server.create_user("admin", Role::Admin).await;
    let mut admin = server.client("192.0.2.1");
    admin.login("admin", PASSWORD).await;
    for days in ["0", "31", "abc"] {
        let response = admin
            .submit("/admin", "/admin/invites", &[("days", days)])
            .await;
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY, "{days}");
    }
}

// --- Administration ---------------------------------------------------------

#[tokio::test]
async fn disabled_user_cannot_log_in_and_loses_sessions() {
    let server = TestServer::new().await;
    server.create_user("admin", Role::Admin).await;
    let bob = server.create_user("bob", Role::User).await;

    let mut bob_browser = server.client("192.0.2.7");
    bob_browser
        .login("bob", PASSWORD)
        .await
        .assert_redirect("/app");

    let mut admin = server.client("192.0.2.1");
    admin.login("admin", PASSWORD).await;
    admin
        .submit("/admin", &format!("/admin/users/{}/disable", bob.id), &[])
        .await
        .assert_redirect("/admin");

    bob_browser.get("/app").await.assert_redirect("/login");
    let response = bob_browser.login("bob", PASSWORD).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    // Enabling restores access.
    admin
        .submit("/admin", &format!("/admin/users/{}/enable", bob.id), &[])
        .await
        .assert_redirect("/admin");
    bob_browser
        .login("bob", PASSWORD)
        .await
        .assert_redirect("/app");
}

#[tokio::test]
async fn admin_cannot_disable_self() {
    let server = TestServer::new().await;
    let admin_user = server.create_user("admin", Role::Admin).await;
    let mut admin = server.client("192.0.2.1");
    admin.login("admin", PASSWORD).await;
    let response = admin
        .submit(
            "/admin",
            &format!("/admin/users/{}/disable", admin_user.id),
            &[],
        )
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(
        !users::get_by_id(&server.state.db, admin_user.id)
            .await
            .unwrap()
            .unwrap()
            .is_disabled()
    );
}

#[tokio::test]
async fn non_admins_cannot_use_admin_pages() {
    let server = TestServer::new().await;
    let admin_user = server.create_user("admin", Role::Admin).await;
    server.create_user("bob", Role::User).await;
    let mut bob = server.client("192.0.2.7");
    bob.login("bob", PASSWORD).await;

    assert_eq!(bob.get("/admin").await.status, StatusCode::FORBIDDEN);
    let csrf = bob.get("/app").await.csrf();
    for path in [
        "/admin/invites".to_owned(),
        format!("/admin/users/{}/disable", admin_user.id),
    ] {
        let response = bob.post(&path, &[("csrf", &csrf), ("days", "7")]).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN, "{path}");
    }
    assert!(
        !users::get_by_id(&server.state.db, admin_user.id)
            .await
            .unwrap()
            .unwrap()
            .is_disabled()
    );

    let mut anonymous = server.client("192.0.2.8");
    anonymous.get("/admin").await.assert_redirect("/login");
}

#[tokio::test]
async fn admin_forms_need_the_session_csrf_token() {
    let server = TestServer::new().await;
    server.create_user("admin", Role::Admin).await;
    let mut admin = server.client("192.0.2.1");
    admin.login("admin", PASSWORD).await;
    let response = admin
        .post(
            "/admin/invites",
            &[("csrf", &"0".repeat(64)), ("days", "7")],
        )
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invites")
        .fetch_one(&server.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn trusted_proxy_forwards_the_client_ip() {
    let server = TestServer::with_env(&[("SCRN_TRUSTED_PROXIES", "192.0.2.1")]).await;
    server.create_user("bob", Role::User).await;
    let mut proxy = server.client("192.0.2.1");
    proxy.headers = vec![("x-forwarded-for".into(), "198.51.100.1".into())];
    for i in 0..LOGIN_ATTEMPTS_PER_IP {
        proxy.login(&format!("user{i}"), "wrong password!!").await;
    }
    assert_eq!(
        proxy.login("bob", PASSWORD).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Another client behind the same proxy has its own budget.
    proxy.headers = vec![("x-forwarded-for".into(), "198.51.100.2".into())];
    proxy.login("bob", PASSWORD).await.assert_redirect("/app");
}
