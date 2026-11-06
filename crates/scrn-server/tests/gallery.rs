//! Integration tests for the gallery and the detail view.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{Client, PASSWORD, START, TestServer, png_image, upload};
use scrn_proto::ScreenshotInfo;
use scrn_server::html;
use scrn_server::repo::screenshots;
use scrn_server::repo::users::{self, Language, Role, Theme, User};
use scrn_server::routes::PAGE_SIZE;
use scrn_server::storage;
use uuid::Uuid;

const DAY: i64 = 86_400;

struct Setup {
    server: TestServer,
    user: User,
    browser: Client,
    api: Client,
}

async fn setup() -> Setup {
    let server = TestServer::new().await;
    let user = server.create_user("alice", Role::User).await;
    let mut browser = server.client("192.0.2.1");
    browser
        .login("alice", PASSWORD)
        .await
        .assert_redirect("/app");
    let api = server.api_client(&user).await;
    Setup {
        server,
        user,
        browser,
        api,
    }
}

async fn shot(api: &mut Client, query: &str) -> ScreenshotInfo {
    let response = upload(api, query, png_image(40, 20)).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    serde_json::from_str(&response.body).unwrap()
}

fn short_path(url: &str) -> String {
    url.strip_prefix("https://scrn.test").unwrap().to_owned()
}

async fn stored(s: &Setup, id: &str) -> screenshots::Screenshot {
    screenshots::get_for_user(&s.server.state.db, Uuid::parse_str(id).unwrap(), s.user.id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn gallery_shows_cards_with_links_only_for_public_images() {
    let mut s = setup().await;
    let empty = s.browser.get("/app").await;
    assert!(empty.body.contains("Noch keine Screenshots"));

    let private = shot(&mut s.api, "").await;
    s.server.clock.advance(10);
    let public = shot(&mut s.api, "?visibility=public").await;
    let url = public.short_url.clone().unwrap();

    let page = s.browser.get("/app").await;
    assert_eq!(page.status, StatusCode::OK);
    // Newest first.
    assert!(page.body.find(&public.id).unwrap() < page.body.find(&private.id).unwrap());
    // The public card has its link with a copy button, the private one none.
    assert!(page.body.contains(&format!(r#"value="{url}""#)));
    assert!(
        page.body
            .contains(&format!(r#"data-copy="url-{}""#, public.id))
    );
    assert!(
        !page
            .body
            .contains(&format!(r#"data-copy="url-{}""#, private.id))
    );
    assert_eq!(page.body.matches("data-copy=").count(), 1);
    // Thumbnails are lazy loaded and come from the owner-only route.
    assert!(page.body.contains(&format!(
        r#"src="/app/shots/{}/thumb" loading="lazy""#,
        private.id
    )));

    let public_only = s.browser.get("/app?filter=public").await;
    assert!(public_only.body.contains(&public.id));
    assert!(!public_only.body.contains(&private.id));
    let private_only = s.browser.get("/app?filter=private").await;
    assert!(private_only.body.contains(&private.id));
    assert!(!private_only.body.contains(&public.id));
}

#[tokio::test]
async fn gallery_pages_with_htmx_fragments() {
    let mut s = setup().await;
    let mut ids = Vec::new();
    for _ in 0..=PAGE_SIZE {
        ids.push(shot(&mut s.api, "").await.id);
        s.server.clock.advance(1);
    }
    let oldest = &ids[0];
    let page = s.browser.get("/app").await;
    assert!(!page.body.contains(oldest.as_str()));
    assert!(
        page.body.contains(r#"hx-get="/app?filter=all&amp;page=2""#)
            || page.body.contains(r#"hx-get="/app?filter=all&#38;page=2""#)
    );

    // htmx asks for the fragment; a browser without JS gets the full page.
    s.browser.headers.push(("hx-request".into(), "true".into()));
    let fragment = s.browser.get("/app?filter=all&page=2").await;
    assert_eq!(fragment.status, StatusCode::OK);
    assert!(!fragment.body.contains("<html"));
    assert!(fragment.body.contains(oldest.as_str()));
    assert_eq!(fragment.body.matches("<li class=\"shot-card\">").count(), 1);
    assert!(!fragment.body.contains("more-slot"));
    s.browser.headers.clear();
    let full = s.browser.get("/app?filter=all&page=2").await;
    assert!(full.body.contains("<html"));
    assert!(full.body.contains(oldest.as_str()));
}

#[tokio::test]
async fn switching_visibility_kills_and_renews_links() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "?visibility=public").await;
    let old_link = short_path(&info.short_url.unwrap());
    let detail = format!("/app/shots/{}", info.id);
    let mut visitor = s.server.client("203.0.113.1");
    assert_eq!(visitor.get(&old_link).await.status, StatusCode::OK);

    // Public → private deletes the short ID.
    s.browser
        .submit(
            &detail,
            &format!("{detail}/visibility"),
            &[("visibility", "private")],
        )
        .await
        .assert_redirect(&detail);
    assert!(stored(&s, &info.id).await.public.is_none());
    assert_eq!(visitor.get(&old_link).await.status, StatusCode::NOT_FOUND);
    let page = s.browser.get(&detail).await;
    assert!(!page.body.contains(&old_link[1..]));
    assert!(!page.body.contains("data-copy"));

    // Private → public creates a new short ID with the user's default
    // expiry; the old link stays dead.
    users::set_preferences(&s.server.state.db, s.user.id, Language::De, Theme::Dark, 7)
        .await
        .unwrap();
    s.browser
        .submit(
            &detail,
            &format!("{detail}/visibility"),
            &[("visibility", "public")],
        )
        .await
        .assert_redirect(&detail);
    let link = stored(&s, &info.id).await.public.unwrap();
    assert_ne!(format!("/{}", link.short_id), old_link);
    assert_eq!(link.expires_at, Some(START + 7 * DAY));
    assert_eq!(
        visitor.get(&format!("/{}", link.short_id)).await.status,
        StatusCode::OK
    );
    assert_eq!(visitor.get(&old_link).await.status, StatusCode::NOT_FOUND);
    assert!(s.browser.get(&detail).await.body.contains(&link.short_id));

    let response = s
        .browser
        .submit(
            &detail,
            &format!("{detail}/visibility"),
            &[("visibility", "secret")],
        )
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
}

async fn expires(s: &Setup, id: &str) -> Option<i64> {
    stored(s, id).await.public.unwrap().expires_at
}

#[tokio::test]
async fn expiry_can_be_changed_for_public_images() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "?visibility=public").await;
    let detail = format!("/app/shots/{}", info.id);
    let action = format!("{detail}/expiry");

    s.browser
        .submit(&detail, &action, &[("expiry", "7")])
        .await
        .assert_redirect(&detail);
    assert_eq!(expires(&s, &info.id).await, Some(START + 7 * DAY));

    // A specific date ends at midnight UTC after that day.
    let date = html::format_date(START + 10 * DAY);
    s.browser
        .submit(
            &detail,
            &action,
            &[("expiry", "custom"), ("custom_date", &date)],
        )
        .await
        .assert_redirect(&detail);
    let end = (START + 10 * DAY).div_euclid(DAY) * DAY + DAY;
    assert_eq!(expires(&s, &info.id).await, Some(end));
    let page = s.browser.get(&detail).await;
    assert!(page.body.contains(&format!(r#"value="{date}""#)));

    s.browser
        .submit(&detail, &action, &[("expiry", "0")])
        .await
        .assert_redirect(&detail);
    assert_eq!(expires(&s, &info.id).await, None);

    for date in [
        html::format_date(START - DAY),
        "2026-02-30".to_owned(),
        html::format_date(START + 4000 * DAY),
    ] {
        let response = s
            .browser
            .submit(
                &detail,
                &action,
                &[("expiry", "custom"), ("custom_date", &date)],
            )
            .await;
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY, "{date}");
        assert!(response.body.contains("in der Zukunft"));
    }
    let response = s
        .browser
        .submit(&detail, &action, &[("expiry", "14")])
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(expires(&s, &info.id).await, None);

    // Private images have no expiry.
    let private = shot(&mut s.api, "").await;
    let detail = format!("/app/shots/{}", private.id);
    let response = s
        .browser
        .submit(&detail, &format!("{detail}/expiry"), &[("expiry", "7")])
        .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert!(stored(&s, &private.id).await.public.is_none());
}

#[tokio::test]
async fn expired_links_show_no_copy_button() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "?visibility=public&expires_in_days=1").await;
    s.server.clock.advance(DAY);
    let page = s.browser.get("/app").await;
    assert!(page.body.contains("Abgelaufen"));
    assert!(!page.body.contains("data-copy"));
    let detail = s.browser.get(&format!("/app/shots/{}", info.id)).await;
    assert!(detail.body.contains("Abgelaufen"));
    assert!(!detail.body.contains("data-copy"));
}

#[tokio::test]
async fn only_the_owner_can_see_or_change_an_image() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "?visibility=public").await;
    let link_before = stored(&s, &info.id).await.public;
    s.server.create_user("mallory", Role::Admin).await;
    let mut other = s.server.client("192.0.2.66");
    other
        .login("mallory", PASSWORD)
        .await
        .assert_redirect("/app");

    let detail = format!("/app/shots/{}", info.id);
    for path in [
        detail.clone(),
        format!("{detail}/image"),
        format!("{detail}/thumb"),
    ] {
        assert_eq!(
            other.get(&path).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert!(!other.get("/app").await.body.contains(&info.id));

    let csrf = other.get("/app").await.csrf();
    for (action, fields) in [
        ("visibility", vec![("visibility", "private")]),
        ("expiry", vec![("expiry", "1")]),
        ("delete", vec![]),
    ] {
        let mut all = vec![("csrf", csrf.as_str())];
        all.extend(fields);
        let response = other.post(&format!("{detail}/{action}"), &all).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{action}");
    }
    assert_eq!(stored(&s, &info.id).await.public, link_before);

    // Logged out: back to the login page.
    let mut anonymous = s.server.client("192.0.2.67");
    anonymous
        .get(&format!("{detail}/image"))
        .await
        .assert_redirect("/login");
    anonymous.get("/app").await.assert_redirect("/login");
    // Malformed IDs are a plain 404.
    assert_eq!(
        s.browser.get("/app/shots/nope").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn owner_gets_image_and_thumbnail() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "").await;
    let id = Uuid::parse_str(&info.id).unwrap();
    let files = s.server.state.config.files_dir();
    for (path, file) in [
        (
            format!("/app/shots/{id}/image"),
            storage::image_path(&files, s.user.id, id),
        ),
        (
            format!("/app/shots/{id}/thumb"),
            storage::thumb_path(&files, s.user.id, id),
        ),
    ] {
        let response = s.browser.get(&path).await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.headers["content-type"], "image/png");
        assert_eq!(response.headers["x-content-type-options"], "nosniff");
        assert_eq!(response.headers["cache-control"], "private, max-age=300");
        assert_eq!(response.bytes, std::fs::read(file).unwrap());
    }
    let page = s.browser.get(&format!("/app/shots/{id}")).await;
    assert!(page.body.contains("40 × 20 px"));
    assert!(page.body.contains("Privat"));
}

#[tokio::test]
async fn delete_removes_row_and_files() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "?visibility=public").await;
    let id = Uuid::parse_str(&info.id).unwrap();
    let files = s.server.state.config.files_dir();
    let detail = format!("/app/shots/{id}");

    let response = s
        .browser
        .post(&format!("{detail}/delete"), &[("csrf", "wrong")])
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert!(storage::image_path(&files, s.user.id, id).exists());

    s.browser
        .submit(&detail, &format!("{detail}/delete"), &[])
        .await
        .assert_redirect("/app");
    assert!(
        screenshots::get_for_user(&s.server.state.db, id, s.user.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!storage::image_path(&files, s.user.id, id).exists());
    assert!(!storage::thumb_path(&files, s.user.id, id).exists());
    assert_eq!(s.browser.get(&detail).await.status, StatusCode::NOT_FOUND);
    let mut visitor = s.server.client("203.0.113.1");
    assert_eq!(
        visitor
            .get(&short_path(&info.short_url.unwrap()))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn views_are_shown_for_public_links() {
    let mut s = setup().await;
    let public = shot(&mut s.api, "?visibility=public").await;
    let private = shot(&mut s.api, "").await;
    let mut visitor = s.server.client("203.0.113.1");
    let link = short_path(public.short_url.as_deref().unwrap());
    for _ in 0..2 {
        assert_eq!(visitor.get(&link).await.status, StatusCode::OK);
    }

    let gallery = s.browser.get("/app").await.body;
    assert!(gallery.contains(r#"title="Aufrufe: 2""#), "{gallery}");
    assert_eq!(gallery.matches(r#"class="views""#).count(), 1);
    let detail = s
        .browser
        .get(&format!("/app/shots/{}", public.id))
        .await
        .body;
    assert!(detail.contains("<dt>Aufrufe</dt><dd>2</dd>"));
    let detail = s
        .browser
        .get(&format!("/app/shots/{}", private.id))
        .await
        .body;
    assert!(!detail.contains("Aufrufe"));

    // Making it private and public again starts a new link from zero.
    let page = format!("/app/shots/{}", public.id);
    let action = format!("{page}/visibility");
    s.browser
        .submit(&page, &action, &[("visibility", "private")])
        .await;
    s.browser
        .submit(&page, &action, &[("visibility", "public")])
        .await;
    assert_eq!(stored(&s, &public.id).await.views, 0);
    let detail = s.browser.get(&page).await.body;
    assert!(detail.contains("<dt>Aufrufe</dt><dd>0</dd>"));
}

#[tokio::test]
async fn a_chosen_date_ends_at_local_midnight_of_the_browser() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "?visibility=public").await;
    let detail = format!("/app/shots/{}", info.id);
    let action = format!("{detail}/expiry");
    let date = html::format_date(START + 10 * DAY);
    let utc_end = (START + 10 * DAY).div_euclid(DAY) * DAY + DAY;

    // Europe/Berlin in winter (UTC+1) and America/Los_Angeles (UTC-8).
    for local_end in [utc_end - 3600, utc_end + 8 * 3600] {
        let end = local_end.to_string();
        s.browser
            .submit(
                &detail,
                &action,
                &[
                    ("expiry", "custom"),
                    ("custom_date", &date),
                    ("custom_end", &end),
                ],
            )
            .await
            .assert_redirect(&detail);
        assert_eq!(expires(&s, &info.id).await, Some(local_end));
    }

    // Values no time zone can produce fall back to midnight UTC.
    for bogus in ["0", "abc", &(utc_end + 15 * 3600).to_string()] {
        s.browser
            .submit(
                &detail,
                &action,
                &[
                    ("expiry", "custom"),
                    ("custom_date", &date),
                    ("custom_end", bogus),
                ],
            )
            .await
            .assert_redirect(&detail);
        assert_eq!(expires(&s, &info.id).await, Some(utc_end), "{bogus}");
    }
}

#[tokio::test]
async fn times_carry_a_machine_readable_utc_value() {
    let mut s = setup().await;
    let info = shot(&mut s.api, "?visibility=public").await;
    let iso = html::format_iso(START);
    let expected = format!(
        r#"<time datetime="{iso}">{}</time>"#,
        html::format_time(START)
    );
    assert!(s.browser.get("/app").await.body.contains(&expected));
    let detail = s.browser.get(&format!("/app/shots/{}", info.id)).await.body;
    assert!(detail.contains(&expected));
    let settings = s.browser.get("/app/settings").await.body;
    assert!(settings.contains(r#"<time datetime=""#));
}
