//! Integration tests for the web UI foundation: language and theme
//! resolution, the settings page, quotas on the admin page and assets.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{Client, PASSWORD, TestServer, png_image, upload};
use scrn_server::html::{PAGE_CSP, asset_url};
use scrn_server::repo::users::{self, Language, Role, Theme};

fn html_tag(lang: &str, theme: &str) -> String {
    format!(r#"<html lang="{lang}" data-theme="{theme}">"#)
}

async fn logged_in(server: &TestServer, name: &str, role: Role) -> (users::User, Client) {
    let user = server.create_user(name, role).await;
    let mut browser = server.client("192.0.2.1");
    browser.login(name, PASSWORD).await.assert_redirect("/app");
    (user, browser)
}

async fn save_settings(browser: &mut Client, fields: &[(&str, &str)]) -> common::TestResponse {
    browser
        .submit("/app/settings", "/app/settings", fields)
        .await
}

#[tokio::test]
async fn first_visit_is_german_and_dark_with_strict_headers() {
    let server = TestServer::new().await;
    server.create_user("alice", Role::User).await;
    let page = server.client("192.0.2.1").get("/login").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(&html_tag("de", "dark")));
    assert!(page.body.contains("Anmelden"));
    assert_eq!(page.headers["content-security-policy"], PAGE_CSP);
    assert_eq!(page.headers["x-frame-options"], "DENY");
    assert_eq!(page.headers["x-content-type-options"], "nosniff");
    // No inline scripts or styles, so the CSP needs no exceptions.
    assert!(!page.body.contains("<script>"));
    assert!(!page.body.contains("style="));
    assert!(page.body.contains(&asset_url("app.css")));
}

#[tokio::test]
async fn accept_language_and_cookies_pick_the_language() {
    let server = TestServer::new().await;
    server.create_user("alice", Role::User).await;

    let mut english = server.client("192.0.2.1");
    english
        .headers
        .push(("accept-language".into(), "en-GB,en;q=0.9,de;q=0.5".into()));
    let page = english.get("/login").await;
    assert!(page.body.contains(&html_tag("en", "dark")));
    assert!(page.body.contains("Sign in"));

    // The cookie wins over the header.
    english.cookies.insert("scrn_lang".into(), "de".into());
    english.cookies.insert("scrn_theme".into(), "light".into());
    let page = english.get("/login").await;
    assert!(page.body.contains(&html_tag("de", "light")));
}

#[tokio::test]
async fn logged_out_switch_stores_cookies() {
    let server = TestServer::new().await;
    server.create_user("alice", Role::User).await;
    let mut browser = server.client("192.0.2.1");

    let response = browser
        .submit(
            "/login",
            "/prefs",
            &[("lang", "en"), ("theme", "light"), ("back", "/login")],
        )
        .await;
    response.assert_redirect("/login");
    assert_eq!(browser.cookies["scrn_lang"], "en");
    assert_eq!(browser.cookies["scrn_theme"], "light");
    assert!(
        browser
            .get("/login")
            .await
            .body
            .contains(&html_tag("en", "light"))
    );

    // Foreign redirect targets are replaced, invalid values rejected.
    browser
        .submit(
            "/login",
            "/prefs",
            &[
                ("lang", "de"),
                ("theme", "dark"),
                ("back", "//evil.example"),
            ],
        )
        .await
        .assert_redirect("/");
    let response = browser
        .submit("/login", "/prefs", &[("lang", "fr"), ("theme", "dark")])
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);

    // CSRF is required.
    let response = browser
        .post(
            "/prefs",
            &[("csrf", "wrong"), ("lang", "en"), ("theme", "dark")],
        )
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn settings_persist_and_follow_the_user() {
    let server = TestServer::new().await;
    let (user, mut browser) = logged_in(&server, "alice", Role::User).await;
    let page = browser.get("/app/settings").await;
    assert!(page.body.contains(&html_tag("de", "dark")));

    let response = save_settings(
        &mut browser,
        &[("lang", "en"), ("theme", "light"), ("expiry", "7")],
    )
    .await;
    response.assert_redirect("/app/settings?saved=1");
    let page = browser.get("/app/settings?saved=1").await;
    assert!(page.body.contains(&html_tag("en", "light")));
    assert!(page.body.contains("Settings saved."));
    assert!(page.body.contains(r#"<option value="7" selected>"#));

    let stored = users::get_by_id(&server.state.db, user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (stored.language, stored.theme),
        (Language::En, Theme::Light)
    );
    assert_eq!(stored.default_expiry_days, 7);

    // The user's setting wins over a stale cookie, and the logged-out pages
    // keep the choice after logout.
    browser.cookies.insert("scrn_lang".into(), "de".into());
    assert!(
        browser
            .get("/app")
            .await
            .body
            .contains(&html_tag("en", "light"))
    );
    let csrf = browser.get("/app").await.csrf();
    browser.post("/logout", &[("csrf", &csrf)]).await;
    browser.cookies.insert("scrn_lang".into(), "en".into());
    assert!(
        browser
            .get("/login")
            .await
            .body
            .contains(&html_tag("en", "light"))
    );

    // A fresh browser gets the user's choice at login.
    let mut other = server.client("192.0.2.2");
    other.login("alice", PASSWORD).await.assert_redirect("/app");
    assert_eq!(other.cookies["scrn_lang"], "en");
    assert_eq!(other.cookies["scrn_theme"], "light");
}

#[tokio::test]
async fn custom_expiry_and_invalid_settings() {
    let server = TestServer::new().await;
    let (user, mut browser) = logged_in(&server, "alice", Role::User).await;

    save_settings(
        &mut browser,
        &[
            ("lang", "de"),
            ("theme", "dark"),
            ("expiry", "custom"),
            ("custom_days", "14"),
        ],
    )
    .await
    .assert_redirect("/app/settings?saved=1");
    let stored = users::get_by_id(&server.state.db, user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.default_expiry_days, 14);
    let page = browser.get("/app/settings").await;
    assert!(page.body.contains(r#"<option value="custom" selected>"#));
    assert!(page.body.contains(r#"value="14""#));

    let response = save_settings(
        &mut browser,
        &[
            ("lang", "de"),
            ("theme", "dark"),
            ("expiry", "custom"),
            ("custom_days", "0"),
        ],
    )
    .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response.body.contains("zwischen 1 und 3650 Tagen"));

    for fields in [
        [("lang", "de"), ("theme", "dark"), ("expiry", "14")],
        [("lang", "fr"), ("theme", "dark"), ("expiry", "0")],
        [("lang", "de"), ("theme", "pink"), ("expiry", "0")],
    ] {
        let response = save_settings(&mut browser, &fields).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{fields:?}");
    }
    let response = browser
        .post(
            "/app/settings",
            &[
                ("csrf", "x"),
                ("lang", "en"),
                ("theme", "dark"),
                ("expiry", "0"),
            ],
        )
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let stored = users::get_by_id(&server.state.db, user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.default_expiry_days, 14);
    assert_eq!(stored.language, Language::De);
}

#[tokio::test]
async fn settings_show_storage_usage() {
    let server = TestServer::new().await;
    let (user, mut browser) = logged_in(&server, "alice", Role::User).await;
    let page = browser.get("/app/settings").await;
    assert!(page.body.contains(r#"<progress max="100" value="0""#));
    assert!(page.body.contains("0 B von 1,0 GiB belegt"));

    let mut api = server.api_client(&user).await;
    assert_eq!(
        upload(&mut api, "", png_image(64, 64)).await.status,
        StatusCode::CREATED
    );
    let page = browser.get("/app/settings").await;
    assert!(!page.body.contains("0 B von 1,0 GiB"));
    assert!(page.body.contains("von 1,0 GiB belegt"));

    users::set_quota(&server.state.db, user.id, Some(0))
        .await
        .unwrap();
    let page = browser.get("/app/settings").await;
    assert!(page.body.contains("belegt, kein Limit"));
    assert!(!page.body.contains("<progress"));
}

#[tokio::test]
async fn new_accounts_keep_the_page_language() {
    let server = TestServer::new().await;
    let (_, mut admin) = logged_in(&server, "admin", Role::Admin).await;
    let page = admin
        .submit("/admin", "/admin/invites", &[("days", "7")])
        .await;
    let start = page.body.find("https://scrn.test/invite/").unwrap() + "https://scrn.test".len();
    let end = page.body[start..].find('"').unwrap();
    let link = page.body[start..start + end].to_owned();

    let mut guest = server.client("192.0.2.9");
    guest.headers.push(("accept-language".into(), "en".into()));
    let page = guest.get(&link).await;
    assert!(page.body.contains(&html_tag("en", "dark")));
    guest
        .submit(
            &link,
            &link,
            &[
                ("username", "bob"),
                ("password", PASSWORD),
                ("password_confirm", PASSWORD),
            ],
        )
        .await
        .assert_redirect("/app");
    let bob = users::get_by_username(&server.state.db, "bob")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bob.language, Language::En);
}

#[tokio::test]
async fn error_pages_use_the_visitor_language() {
    let server = TestServer::new().await;
    let mut browser = server.client("192.0.2.1");
    let page = browser.get("/does/not/exist").await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert!(page.body.contains(&html_tag("de", "dark")));
    assert!(page.body.contains("Die Seite wurde nicht gefunden."));
    assert_eq!(page.headers["content-security-policy"], PAGE_CSP);

    browser.cookies.insert("scrn_lang".into(), "en".into());
    browser.cookies.insert("scrn_theme".into(), "light".into());
    let page = browser.get("/does/not/exist").await;
    assert!(page.body.contains(&html_tag("en", "light")));
    assert!(page.body.contains("The page was not found."));
}

#[tokio::test]
async fn admin_sets_quotas_and_sees_usage() {
    let server = TestServer::with_env(&[("SCRN_DEFAULT_QUOTA_MB", "10")]).await;
    let (_, mut admin) = logged_in(&server, "admin", Role::Admin).await;
    let bob = server.create_user("bob", Role::User).await;
    let mut api = server.api_client(&bob).await;
    upload(&mut api, "", png_image(64, 64)).await;

    let page = admin.get("/admin").await;
    assert!(page.body.contains("Serverstandard (10,0 MiB)"));
    assert!(page.body.contains("von 10,0 MiB belegt"));

    let path = format!("/admin/users/{}/quota", bob.id);
    let quota = || async {
        users::get_by_id(&server.state.db, bob.id)
            .await
            .unwrap()
            .unwrap()
            .quota_bytes
    };
    admin
        .submit("/admin", &path, &[("quota_mb", "5")])
        .await
        .assert_redirect("/admin");
    assert_eq!(quota().await, Some(5 * 1024 * 1024));
    let page = admin.get("/admin").await;
    assert!(page.body.contains(r#"value="5""#));
    assert!(page.body.contains("von 5,0 MiB belegt"));

    admin
        .submit("/admin", &path, &[("quota_mb", "0")])
        .await
        .assert_redirect("/admin");
    assert_eq!(quota().await, Some(0));
    assert!(
        admin
            .get("/admin")
            .await
            .body
            .contains("belegt, kein Limit")
    );

    admin
        .submit("/admin", &path, &[("quota_mb", "")])
        .await
        .assert_redirect("/admin");
    assert_eq!(quota().await, None);

    let response = admin.submit("/admin", &path, &[("quota_mb", "-3")]).await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response.body.contains("ganze Zahl ab 0"));
    assert_eq!(quota().await, None);

    let response = admin
        .submit("/admin", "/admin/users/999/quota", &[("quota_mb", "1")])
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    let response = admin
        .post(&path, &[("csrf", "wrong"), ("quota_mb", "1")])
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);

    // Regular users cannot change quotas.
    let mut bob_browser = server.client("192.0.2.5");
    bob_browser.login("bob", PASSWORD).await;
    let csrf = bob_browser.get("/app").await.csrf();
    let response = bob_browser
        .post(&path, &[("csrf", &csrf), ("quota_mb", "0")])
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(quota().await, None);
}

#[tokio::test]
async fn static_assets_are_served_with_cache_headers() {
    let server = TestServer::new().await;
    let mut browser = server.client("192.0.2.1");
    for (name, content_type) in [
        ("app.css", "text/css; charset=utf-8"),
        ("htmx.min.js", "text/javascript; charset=utf-8"),
        ("favicon.svg", "image/svg+xml"),
    ] {
        let response = browser.get(&asset_url(name)).await;
        assert_eq!(response.status, StatusCode::OK, "{name}");
        assert_eq!(response.headers["content-type"], content_type);
        assert_eq!(
            response.headers["cache-control"],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(response.headers["x-content-type-options"], "nosniff");
    }
    let response = browser.get("/static/app.css").await;
    assert_eq!(response.headers["cache-control"], "no-cache");
    assert_eq!(
        browser.get("/static/secret.txt").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        browser.get("/static/../Cargo.toml").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn footer_shows_version_and_source_link() {
    let server = TestServer::new().await;
    let page = server.client("192.0.2.50").get("/login").await.body;
    assert!(page.contains(&format!(
        r#"<span class="version">v{}</span>"#,
        env!("CARGO_PKG_VERSION")
    )));
    assert!(page.contains(r#"href="https://github.com/andreaslink-de/scrn""#));
    assert!(page.contains(r#"aria-label="Quellcode auf GitHub""#));
}
