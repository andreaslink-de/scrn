//! Integration tests for device groups: managing them in the settings,
//! assigning API tokens and filtering the gallery by group.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{Client, PASSWORD, START, TestServer, png_image, upload};
use scrn_proto::ScreenshotInfo;
use scrn_server::crypto::ApiTokenSecret;
use scrn_server::repo::screenshots::{self, NewScreenshot};
use scrn_server::repo::users::{Role, User};
use scrn_server::repo::{api_tokens, device_groups};

struct Setup {
    server: TestServer,
    user: User,
    browser: Client,
}

async fn setup() -> Setup {
    let server = TestServer::new().await;
    let user = server.create_user("alice", Role::User).await;
    let mut browser = server.client("192.0.2.1");
    browser
        .login("alice", PASSWORD)
        .await
        .assert_redirect("/app");
    Setup {
        server,
        user,
        browser,
    }
}

async fn create_group(browser: &mut Client, name: &str) -> common::TestResponse {
    browser
        .submit("/app/settings", "/app/groups", &[("name", name)])
        .await
}

/// The ID of the user's group with this name.
async fn group_id(s: &Setup, name: &str) -> i64 {
    device_groups::list_for_user(&s.server.state.db, s.user.id)
        .await
        .unwrap()
        .into_iter()
        .find(|g| g.name.as_deref() == Some(name))
        .expect("group exists")
        .id
}

async fn default_group_id(s: &Setup) -> i64 {
    device_groups::list_for_user(&s.server.state.db, s.user.id)
        .await
        .unwrap()
        .into_iter()
        .find(|g| g.is_default)
        .unwrap()
        .id
}

/// Creates a token through the settings page and returns an API client
/// using it.
async fn token_in(s: &mut Setup, name: &str, group: Option<i64>) -> Client {
    let group = group.map(|g| g.to_string());
    let mut fields = vec![("name", name)];
    if let Some(group) = &group {
        fields.push(("group", group));
    }
    let page = s
        .browser
        .submit("/app/settings", "/app/tokens", &fields)
        .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let marker = r#"id="new-token" type="text" readonly value=""#;
    let start = page.body.find(marker).expect("token shown") + marker.len();
    let end = page.body[start..].find('"').unwrap();
    let token = &page.body[start..start + end];
    let mut api = s.server.client("198.51.100.1");
    api.headers
        .push(("authorization".into(), format!("Bearer {token}")));
    api
}

async fn shot(api: &mut Client) -> String {
    let response = upload(api, "", png_image(40, 20)).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    serde_json::from_str::<ScreenshotInfo>(&response.body)
        .unwrap()
        .id
}

fn shows(page: &common::TestResponse, id: &str) -> bool {
    page.body.contains(&format!("/app/shots/{id}/thumb"))
}

#[tokio::test]
async fn one_group_means_no_group_filter() {
    let mut s = setup().await;
    let settings = s.browser.get("/app/settings").await;
    assert!(settings.body.contains(r#"id="groups""#));
    assert!(settings.body.contains(r#"value="Standard""#));
    assert!(settings.body.contains("Standardgruppe"));
    assert!(!settings.body.contains(r#"name="group""#));

    let gallery = s.browser.get("/app").await;
    assert!(!gallery.body.contains("Alle Geräte"));
}

#[tokio::test]
async fn create_and_rename_groups() {
    let mut s = setup().await;
    create_group(&mut s.browser, " Arbeit ")
        .await
        .assert_redirect("/app/settings#groups");
    let work = group_id(&s, "Arbeit").await;

    let taken = create_group(&mut s.browser, "ARBEIT").await;
    assert_eq!(taken.status, StatusCode::CONFLICT);
    assert!(taken.body.contains("bereits eine Gruppe"));
    let empty = create_group(&mut s.browser, "  ").await;
    assert_eq!(empty.status, StatusCode::UNPROCESSABLE_ENTITY);
    let long = create_group(&mut s.browser, &"x".repeat(51)).await;
    assert_eq!(long.status, StatusCode::UNPROCESSABLE_ENTITY);

    s.browser
        .submit(
            "/app/settings",
            &format!("/app/groups/{work}/rename"),
            &[("name", "Büro")],
        )
        .await
        .assert_redirect("/app/settings#groups");
    assert_eq!(group_id(&s, "Büro").await, work);

    // The default group can be renamed, too.
    let default = default_group_id(&s).await;
    s.browser
        .submit(
            "/app/settings",
            &format!("/app/groups/{default}/rename"),
            &[("name", "Zu Hause")],
        )
        .await
        .assert_redirect("/app/settings#groups");
    let renamed = s
        .browser
        .submit(
            "/app/settings",
            &format!("/app/groups/{work}/rename"),
            &[("name", "zu hause")],
        )
        .await;
    assert_eq!(renamed.status, StatusCode::CONFLICT);

    let settings = s.browser.get("/app/settings").await;
    assert!(settings.body.contains(r#"value="Zu Hause""#));
    assert!(settings.body.contains(r#"value="Büro""#));
}

#[tokio::test]
async fn gallery_filters_by_the_group_of_the_uploading_token() {
    let mut s = setup().await;
    // A screenshot from before groups existed: no token known.
    let old = uuid::Uuid::new_v4();
    screenshots::insert(
        &s.server.state.db,
        &NewScreenshot {
            id: old,
            user_id: s.user.id,
            token_id: None,
            public: None,
            width: 40,
            height: 20,
            bytes: 10,
            thumb_bytes: 10,
            sha256: [1; 32],
        },
        START,
    )
    .await
    .unwrap();
    let old = old.to_string();

    create_group(&mut s.browser, "Arbeit")
        .await
        .assert_redirect("/app/settings#groups");
    let work = group_id(&s, "Arbeit").await;
    let default = default_group_id(&s).await;

    let settings = s.browser.get("/app/settings").await;
    assert!(
        settings
            .body
            .contains(r#"<select id="token-group" name="group""#)
    );
    let mut work_pc = token_in(&mut s, "Arbeits-PC", Some(work)).await;
    let mut home_pc = token_in(&mut s, "Zuhause-PC", None).await;
    let from_work = shot(&mut work_pc).await;
    let from_home = shot(&mut home_pc).await;

    let all = s.browser.get("/app").await;
    assert!(all.body.contains("Alle Geräte"));
    assert!(all.body.contains(&format!(r#"href="/app?group={work}""#)));
    for id in [&old, &from_work, &from_home] {
        assert!(shows(&all, id));
    }

    let work_page = s.browser.get(&format!("/app?group={work}")).await;
    assert!(shows(&work_page, &from_work));
    assert!(!shows(&work_page, &from_home));
    assert!(!shows(&work_page, &old));
    // The visibility filter keeps the chosen group.
    assert!(
        work_page
            .body
            .contains(&format!(r#"href="/app?filter=private&amp;group={work}""#))
            || work_page
                .body
                .contains(&format!(r#"href="/app?filter=private&#38;group={work}""#))
    );
    let private_work = s
        .browser
        .get(&format!("/app?filter=private&group={work}"))
        .await;
    assert!(shows(&private_work, &from_work));

    let default_page = s.browser.get(&format!("/app?group={default}")).await;
    assert!(shows(&default_page, &from_home));
    assert!(shows(&default_page, &old));
    assert!(!shows(&default_page, &from_work));

    // An unknown group shows everything.
    let unknown = s.browser.get("/app?group=9999").await;
    assert!(shows(&unknown, &from_work) && shows(&unknown, &from_home));

    // Moving the token moves its screenshots.
    let token_id = api_tokens::list_for_user(&s.server.state.db, s.user.id)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.name == "Arbeits-PC")
        .unwrap()
        .id;
    s.browser
        .submit(
            "/app/settings",
            &format!("/app/tokens/{token_id}/group"),
            &[("group", &default.to_string())],
        )
        .await
        .assert_redirect("/app/settings#tokens");
    let work_page = s.browser.get(&format!("/app?group={work}")).await;
    assert!(!shows(&work_page, &from_work));
    let default_page = s.browser.get(&format!("/app?group={default}")).await;
    assert!(shows(&default_page, &from_work));
}

#[tokio::test]
async fn deleting_a_group_moves_its_tokens_to_the_default_group() {
    let mut s = setup().await;
    create_group(&mut s.browser, "Arbeit")
        .await
        .assert_redirect("/app/settings#groups");
    let work = group_id(&s, "Arbeit").await;
    let default = default_group_id(&s).await;
    token_in(&mut s, "Arbeits-PC", Some(work)).await;

    let not_default = s
        .browser
        .submit(
            "/app/settings",
            &format!("/app/groups/{default}/delete"),
            &[],
        )
        .await;
    assert_eq!(not_default.status, StatusCode::NOT_FOUND);

    s.browser
        .submit("/app/settings", &format!("/app/groups/{work}/delete"), &[])
        .await
        .assert_redirect("/app/settings#groups");
    let tokens = api_tokens::list_for_user(&s.server.state.db, s.user.id)
        .await
        .unwrap();
    assert_eq!(tokens[0].group_id, default);
    // Back to one group: no filter in the gallery.
    assert!(!s.browser.get("/app").await.body.contains("Alle Geräte"));
}

#[tokio::test]
async fn groups_of_other_users_are_out_of_reach() {
    let mut s = setup().await;
    create_group(&mut s.browser, "Arbeit")
        .await
        .assert_redirect("/app/settings#groups");
    let alices_group = group_id(&s, "Arbeit").await;

    let bob = s.server.create_user("bob", Role::User).await;
    let secret = ApiTokenSecret::generate().unwrap();
    let bobs_token =
        api_tokens::create(&s.server.state.db, bob.id, None, "bob", &secret.hash, START)
            .await
            .unwrap();
    let mut browser = s.server.client("192.0.2.2");
    browser.login("bob", PASSWORD).await.assert_redirect("/app");

    for (path, fields) in [
        (
            format!("/app/groups/{alices_group}/rename"),
            vec![("name", "Mine")],
        ),
        (format!("/app/groups/{alices_group}/delete"), vec![]),
    ] {
        let response = browser.submit("/app/settings", &path, &fields).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
    }
    let group = alices_group.to_string();
    let moved = browser
        .submit(
            "/app/settings",
            &format!("/app/tokens/{}/group", bobs_token.id),
            &[("group", &group)],
        )
        .await;
    assert_eq!(moved.status, StatusCode::NOT_FOUND);
    let created = browser
        .submit(
            "/app/settings",
            "/app/tokens",
            &[("name", "sneaky"), ("group", &group)],
        )
        .await;
    assert_eq!(created.status, StatusCode::BAD_REQUEST);
    // Bob's gallery ignores Alice's group.
    let gallery = browser.get(&format!("/app?group={alices_group}")).await;
    assert_eq!(gallery.status, StatusCode::OK);
    assert!(!gallery.body.contains("Arbeit"));
    assert_eq!(group_id(&s, "Arbeit").await, alices_group);
}

#[tokio::test]
async fn group_forms_need_csrf() {
    let mut s = setup().await;
    let forged = s
        .browser
        .post("/app/groups", &[("csrf", "forged"), ("name", "Arbeit")])
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    let mut visitor = s.server.client("192.0.2.9");
    visitor
        .post("/app/groups", &[("csrf", "x"), ("name", "Arbeit")])
        .await
        .assert_redirect("/login");
    assert_eq!(
        device_groups::list_for_user(&s.server.state.db, s.user.id)
            .await
            .unwrap()
            .len(),
        1
    );
}
