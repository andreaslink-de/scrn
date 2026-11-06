//! Integration tests for link expiry and the maintenance job.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, SystemTime};

use axum::http::StatusCode;
use common::{Client, PASSWORD, TestServer, png_image, upload};
use scrn_proto::{ScreenshotInfo, ScreenshotList, Visibility};
use scrn_server::auth::SESSION_TTL;
use scrn_server::jobs::{self, ORPHAN_MIN_AGE};
use scrn_server::repo::users::{self, Language, Role, Theme, User};
use scrn_server::storage;
use uuid::Uuid;

const DAY: i64 = 86_400;

struct Setup {
    server: TestServer,
    user: User,
    api: Client,
    visitor: Client,
}

async fn setup() -> Setup {
    let server = TestServer::new().await;
    let user = server.create_user("alice", Role::User).await;
    let api = server.api_client(&user).await;
    let visitor = server.client("203.0.113.1");
    Setup {
        server,
        user,
        api,
        visitor,
    }
}

async fn shot(api: &mut Client, query: &str) -> ScreenshotInfo {
    let response = upload(api, query, png_image(8, 8)).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    serde_json::from_str(&response.body).unwrap()
}

fn path(info: &ScreenshotInfo) -> String {
    info.short_url
        .as_ref()
        .unwrap()
        .strip_prefix("https://scrn.test")
        .unwrap()
        .to_owned()
}

async fn listed(api: &mut Client, id: &str) -> ScreenshotInfo {
    let list: ScreenshotList =
        serde_json::from_str(&api.get("/api/v1/screenshots?limit=100").await.body).unwrap();
    list.screenshots.into_iter().find(|s| s.id == id).unwrap()
}

async fn pass(s: &Setup) -> jobs::Report {
    jobs::run_once(&s.server.state).await.unwrap()
}

#[tokio::test]
async fn expired_links_become_private_and_stay_in_the_gallery() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "?visibility=public&expires_in_days=1").await;
    let link = path(&info);

    assert_eq!(pass(&s).await.expired_links, 0);
    s.server.clock.advance(DAY - 1);
    assert_eq!(s.visitor.get(&link).await.status, StatusCode::OK);

    // At the expiry the request-time check already refuses the link, even
    // before the job has run.
    s.server.clock.advance(1);
    assert_eq!(s.visitor.get(&link).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        listed(&mut s.api, &info.id).await.visibility,
        Visibility::Public
    );

    assert_eq!(pass(&s).await.expired_links, 1);
    let now = listed(&mut s.api, &info.id).await;
    assert_eq!(now.visibility, Visibility::Private);
    assert_eq!(now.short_url, None);
    assert_eq!(now.expires_at, None);
    assert_eq!(s.visitor.get(&link).await.status, StatusCode::NOT_FOUND);

    // The image is still in the gallery, now as private, with its files.
    let mut browser = s.server.client("192.0.2.1");
    browser.login("alice", PASSWORD).await;
    let gallery = browser.get("/app?filter=private").await;
    assert!(gallery.body.contains(&info.id));
    let id = Uuid::parse_str(&info.id).unwrap();
    assert!(storage::image_path(&s.server.state.config.files_dir(), s.user.id, id).exists());
    assert_eq!(pass(&s).await.expired_links, 0);
}

#[tokio::test]
async fn never_expiring_links_survive_the_job() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "?visibility=public").await;
    assert_eq!(info.expires_at, None);
    s.server.clock.advance(20 * 365 * DAY);
    assert_eq!(pass(&s).await.expired_links, 0);
    assert_eq!(s.visitor.get(&path(&info)).await.status, StatusCode::OK);
}

#[tokio::test]
async fn per_image_expiry_overrides_the_default() {
    let mut s = setup().await;
    users::set_preferences(&s.server.state.db, s.user.id, Language::De, Theme::Dark, 1)
        .await
        .unwrap();
    let mut api = s.server.api_client(&s.user).await;
    let default = shot(&mut api, "?visibility=public").await;
    let never = shot(&mut api, "?visibility=public&expires_in_days=0").await;
    let week = shot(&mut api, "?visibility=public&expires_in_days=7").await;

    // Changed in the gallery: from one day to never.
    let mut browser = s.server.client("192.0.2.1");
    browser.login("alice", PASSWORD).await;
    let extended = shot(&mut api, "?visibility=public").await;
    let extended_detail = format!("/app/shots/{}", extended.id);
    browser
        .submit(
            &extended_detail,
            &format!("{extended_detail}/expiry"),
            &[("expiry", "0")],
        )
        .await
        .assert_redirect(&extended_detail);

    s.server.clock.advance(DAY);
    assert_eq!(pass(&s).await.expired_links, 1);
    assert_eq!(
        listed(&mut api, &default.id).await.visibility,
        Visibility::Private
    );
    for info in [&never, &week, &extended] {
        assert_eq!(s.visitor.get(&path(info)).await.status, StatusCode::OK);
    }

    s.server.clock.advance(6 * DAY);
    assert_eq!(pass(&s).await.expired_links, 1);
    assert_eq!(
        listed(&mut api, &week.id).await.visibility,
        Visibility::Private
    );
    assert_eq!(s.visitor.get(&path(&never)).await.status, StatusCode::OK);
    assert_eq!(s.visitor.get(&path(&extended)).await.status, StatusCode::OK);
}

#[tokio::test]
async fn changing_the_default_only_affects_new_uploads() {
    let mut s = setup().await;
    users::set_preferences(&s.server.state.db, s.user.id, Language::De, Theme::Dark, 30)
        .await
        .unwrap();
    let mut api = s.server.api_client(&s.user).await;
    let old = shot(&mut api, "?visibility=public").await;
    let old_expiry = old.expires_at.unwrap();

    users::set_preferences(&s.server.state.db, s.user.id, Language::De, Theme::Dark, 1)
        .await
        .unwrap();
    let mut api = s.server.api_client(&s.user).await;
    let new = shot(&mut api, "?visibility=public").await;
    assert_eq!(listed(&mut api, &old.id).await.expires_at, Some(old_expiry));
    assert!(new.expires_at.unwrap() < old_expiry);

    s.server.clock.advance(2 * DAY);
    assert_eq!(pass(&s).await.expired_links, 1);
    assert_eq!(s.visitor.get(&path(&old)).await.status, StatusCode::OK);
}

#[tokio::test]
async fn expired_sessions_are_removed() {
    let s = setup().await;
    let mut browser = s.server.client("192.0.2.1");
    browser
        .login("alice", PASSWORD)
        .await
        .assert_redirect("/app");
    assert_eq!(pass(&s).await.expired_sessions, 0);
    s.server.clock.advance(SESSION_TTL);
    assert_eq!(pass(&s).await.expired_sessions, 1);
    browser.get("/app").await.assert_redirect("/login");
}

#[tokio::test]
async fn orphaned_files_are_removed_after_an_hour() {
    let mut s = setup().await;
    let alice = s.user.id;
    let bob = s.server.create_user("bob", Role::User).await.id;
    let kept = shot(&mut s.api, "").await;
    let kept_id = Uuid::parse_str(&kept.id).unwrap();
    let dir = s.server.state.config.files_dir();
    let orphan = Uuid::new_v4();
    std::fs::create_dir_all(storage::user_dir(&dir, bob)).unwrap();
    std::fs::write(storage::image_path(&dir, alice, orphan), b"x").unwrap();
    std::fs::write(storage::thumb_path(&dir, alice, orphan), b"x").unwrap();
    // A copy of Alice's image in Bob's directory can never be served.
    std::fs::write(storage::image_path(&dir, bob, kept_id), b"x").unwrap();
    // Left over from the flat layout: without a screenshot it goes, with
    // one it stays until the startup move picks it up.
    std::fs::write(dir.join(format!("{orphan}.png")), b"x").unwrap();
    std::fs::write(dir.join(format!("{kept_id}.thumb.png")), b"x").unwrap();
    // Files and directories we did not write are never touched.
    std::fs::write(dir.join("notes.txt"), b"keep me").unwrap();
    std::fs::create_dir_all(dir.join("lost+found")).unwrap();
    std::fs::write(dir.join("lost+found").join(format!("{orphan}.png")), b"x").unwrap();
    std::fs::write(storage::user_dir(&dir, alice).join("notes.txt"), b"keep me").unwrap();

    // Fresh files may belong to an upload in progress.
    assert_eq!(pass(&s).await.orphan_files, 0);
    assert!(storage::image_path(&dir, alice, orphan).exists());

    let later = SystemTime::now() + ORPHAN_MIN_AGE + Duration::from_secs(1);
    let removed = jobs::remove_orphans(&s.server.state, &dir, later, ORPHAN_MIN_AGE)
        .await
        .unwrap();
    assert_eq!(removed, 4);
    assert!(!storage::image_path(&dir, alice, orphan).exists());
    assert!(!storage::thumb_path(&dir, alice, orphan).exists());
    assert!(!storage::image_path(&dir, bob, kept_id).exists());
    assert!(!dir.join(format!("{orphan}.png")).exists());
    assert!(dir.join(format!("{kept_id}.thumb.png")).exists());
    assert!(storage::image_path(&dir, alice, kept_id).exists());
    assert!(storage::thumb_path(&dir, alice, kept_id).exists());
    assert!(dir.join("notes.txt").exists());
    assert!(
        dir.join("lost+found")
            .join(format!("{orphan}.png"))
            .exists()
    );
    assert!(storage::user_dir(&dir, alice).join("notes.txt").exists());
}

#[tokio::test]
async fn files_of_the_flat_layout_move_into_user_directories() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "?visibility=public").await;
    let id = Uuid::parse_str(&info.id).unwrap();
    let dir = s.server.state.config.files_dir();
    let image = storage::image_path(&dir, s.user.id, id);
    let thumb = storage::thumb_path(&dir, s.user.id, id);
    let data = std::fs::read(&image).unwrap();
    // Put the files where the flat layout stored them.
    let flat_image = dir.join(format!("{id}.png"));
    let flat_thumb = dir.join(format!("{id}.thumb.png"));
    std::fs::rename(&image, &flat_image).unwrap();
    std::fs::rename(&thumb, &flat_thumb).unwrap();
    std::fs::remove_dir(storage::user_dir(&dir, s.user.id)).unwrap();
    let unknown = dir.join(format!("{}.png", Uuid::new_v4()));
    std::fs::write(&unknown, b"x").unwrap();
    assert_eq!(
        s.visitor.get(&path(&info)).await.status,
        StatusCode::NOT_FOUND
    );

    let moved = jobs::move_legacy_files(&s.server.state.db, &dir)
        .await
        .unwrap();
    assert_eq!(moved, 2);
    assert!(!flat_image.exists() && !flat_thumb.exists());
    assert_eq!(std::fs::read(&image).unwrap(), data);
    assert!(thumb.exists());
    // Files without a screenshot are left for the orphan cleanup.
    assert!(unknown.exists());
    let response = s.visitor.get(&path(&info)).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.bytes, data);

    // Running it again changes nothing.
    assert_eq!(
        jobs::move_legacy_files(&s.server.state.db, &dir)
            .await
            .unwrap(),
        0
    );
}
