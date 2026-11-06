//! Integration tests for the upload API: upload, validation, quotas, listing
//! and deletion.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{Client, START, TestResponse, TestServer};
use png::{BitDepth, ColorType};
use scrn_proto::{ApiErrorBody, MeResponse, ScreenshotInfo, ScreenshotList, Visibility};
use scrn_server::repo::users::{self, Role, User};
use scrn_server::{short_id, storage};
use uuid::Uuid;

const DAY: i64 = 86_400;

/// An RGB test image with a text chunk that must not survive the upload.
fn png_image(width: u32, height: u32) -> Vec<u8> {
    let pixels: Vec<u8> = (0..width * height * 3).map(|i| (i % 251) as u8).collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(ColorType::Rgb);
    encoder.set_depth(BitDepth::Eight);
    encoder
        .add_text_chunk("Comment".into(), "secret-metadata".into())
        .unwrap();
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    out
}

fn decode_pixels(data: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(data))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// A PNG that declares `width` × `height` but contains no pixel data.
fn png_header_only(width: u32, height: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    for (kind, body) in [(&b"IHDR"[..], ihdr.as_slice()), (b"IDAT", &[])] {
        out.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
        let mut chunk = kind.to_vec();
        chunk.extend_from_slice(body);
        out.extend_from_slice(&chunk);
        let mut crc = 0xffff_ffffu32;
        for &b in &chunk {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        out.extend_from_slice(&(!crc).to_be_bytes());
    }
    out
}

async fn upload(api: &mut Client, query: &str, body: Vec<u8>) -> TestResponse {
    api.send_raw(
        "POST",
        &format!("/api/v1/screenshots{query}"),
        Some("image/png"),
        body,
    )
    .await
}

fn created(response: &TestResponse) -> ScreenshotInfo {
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    serde_json::from_str(&response.body).unwrap()
}

fn error(response: &TestResponse, status: StatusCode) -> ApiErrorBody {
    assert_eq!(response.status, status, "{}", response.body);
    serde_json::from_str(&response.body).unwrap()
}

async fn setup() -> (TestServer, User, Client) {
    let server = TestServer::new().await;
    let user = server.create_user("alice", Role::User).await;
    let api = server.api_client(&user).await;
    (server, user, api)
}

fn stored_files(server: &TestServer) -> usize {
    std::fs::read_dir(server.state.config.files_dir())
        .unwrap()
        .count()
}

async fn me(api: &mut Client) -> MeResponse {
    let response = api.get("/api/v1/me").await;
    assert_eq!(response.status, StatusCode::OK);
    serde_json::from_str(&response.body).unwrap()
}

#[tokio::test]
async fn public_upload_returns_a_short_url_and_stores_clean_files() {
    let (server, _, mut api) = setup().await;
    let original = png_image(640, 360);
    let info = created(&upload(&mut api, "?visibility=public", original.clone()).await);

    assert_eq!(info.visibility, Visibility::Public);
    assert_eq!((info.width, info.height), (640, 360));
    assert_eq!(info.expires_at, None);
    assert_eq!(info.created_at, START);
    let short_url = info.short_url.clone().unwrap();
    let short = short_url.strip_prefix("https://scrn.test/").unwrap();
    assert!(short_id::is_well_formed(short), "{short}");

    // The stored file has the same pixels but none of the metadata.
    let id = Uuid::parse_str(&info.id).unwrap();
    let files = server.state.config.files_dir();
    let stored = std::fs::read(storage::image_path(&files, id)).unwrap();
    assert!(!stored.windows(15).any(|w| w == b"secret-metadata"));
    assert!(!stored.windows(4).any(|w| w == b"tEXt"));
    assert_eq!(decode_pixels(&stored), decode_pixels(&original));
    let thumb = std::fs::read(storage::thumb_path(&files, id)).unwrap();
    let (tw, th, _) = decode_pixels(&thumb);
    assert_eq!((tw, th), (480, 270));

    // The client re-copies the last link with limit=1.
    let response = api.get("/api/v1/screenshots?limit=1").await;
    assert_eq!(response.status, StatusCode::OK);
    let list: ScreenshotList = serde_json::from_str(&response.body).unwrap();
    assert_eq!(list.screenshots, vec![info]);

    let me = me(&mut api).await;
    assert_eq!(
        me.used_bytes,
        u64::try_from(stored.len() + thumb.len()).unwrap()
    );
    assert_eq!(me.quota_bytes, Some(1024 * 1024 * 1024));
}

#[tokio::test]
async fn uploads_are_private_by_default() {
    let (_server, _, mut api) = setup().await;
    let info = created(&upload(&mut api, "", png_image(4, 4)).await);
    assert_eq!(info.visibility, Visibility::Private);
    assert_eq!(info.short_url, None);
    assert!(
        !api.get("/api/v1/screenshots")
            .await
            .body
            .contains("short_url")
    );

    let body = error(
        &upload(&mut api, "?expires_in_days=3", png_image(4, 4)).await,
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(body.error, "invalid_request");
    let body = error(
        &upload(&mut api, "?visibility=secret", png_image(4, 4)).await,
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(body.error, "invalid_request");
}

#[tokio::test]
async fn link_expiry_uses_the_parameter_or_the_user_default() {
    let (server, user, mut api) = setup().await;
    let info = created(
        &upload(
            &mut api,
            "?visibility=public&expires_in_days=7",
            png_image(4, 4),
        )
        .await,
    );
    assert_eq!(info.expires_at, Some(START + 7 * DAY));

    sqlx::query("UPDATE users SET default_expiry_days = 30 WHERE id = ?")
        .bind(user.id)
        .execute(&server.state.db)
        .await
        .unwrap();
    let mut api = server.api_client(&user).await;
    let info = created(&upload(&mut api, "?visibility=public", png_image(4, 4)).await);
    assert_eq!(info.expires_at, Some(START + 30 * DAY));

    // 0 means never, even if the user has a default.
    let info = created(
        &upload(
            &mut api,
            "?visibility=public&expires_in_days=0",
            png_image(4, 4),
        )
        .await,
    );
    assert_eq!(info.expires_at, None);

    let body = error(
        &upload(
            &mut api,
            "?visibility=public&expires_in_days=9999",
            png_image(4, 4),
        )
        .await,
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(body.error, "invalid_request");
}

#[tokio::test]
async fn non_png_data_is_rejected() {
    let (server, _, mut api) = setup().await;
    let body = error(
        &upload(&mut api, "", b"just some text, named test.png".to_vec()).await,
        StatusCode::UNPROCESSABLE_ENTITY,
    );
    assert_eq!(body.error, "invalid_image");

    // A real PNG with the wrong declared type is rejected as well.
    for content_type in [None, Some("image/jpeg"), Some("application/octet-stream")] {
        let response = api
            .send_raw("POST", "/api/v1/screenshots", content_type, png_image(4, 4))
            .await;
        let body = error(&response, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body.error, "unsupported_media_type");
    }
    assert_eq!(stored_files(&server), 0);
}

#[tokio::test]
async fn oversized_uploads_are_rejected() {
    let (server, _, mut api) = setup().await;
    let body = error(
        &upload(&mut api, "", png_header_only(20_000, 20_000)).await,
        StatusCode::PAYLOAD_TOO_LARGE,
    );
    assert_eq!(body.error, "image_too_large");

    let body = error(
        &upload(&mut api, "", vec![0; 20 * 1024 * 1024 + 1]).await,
        StatusCode::PAYLOAD_TOO_LARGE,
    );
    assert_eq!(body.error, "payload_too_large");
    assert_eq!(stored_files(&server), 0);
}

#[tokio::test]
async fn quota_blocks_uploads_but_never_deletes() {
    let (server, user, mut api) = setup().await;
    users::set_quota(&server.state.db, user.id, Some(1))
        .await
        .unwrap();
    let body = error(
        &upload(&mut api, "", png_image(4, 4)).await,
        StatusCode::PAYLOAD_TOO_LARGE,
    );
    assert_eq!(body.error, "quota_exceeded");
    assert_eq!(body.used_bytes, Some(0));
    assert_eq!(body.quota_bytes, Some(1));
    assert_eq!(stored_files(&server), 0);

    // Unlimited for this user.
    users::set_quota(&server.state.db, user.id, Some(0))
        .await
        .unwrap();
    let mut api = server.api_client(&user).await;
    let first = created(&upload(&mut api, "", png_image(64, 64)).await);
    created(&upload(&mut api, "", png_image(64, 64)).await);
    let used = me(&mut api).await.used_bytes;
    assert_eq!(me(&mut api).await.quota_bytes, None);

    // Lowering the quota below the usage keeps everything but blocks new
    // uploads.
    let limit = i64::try_from(used).unwrap() - 1;
    users::set_quota(&server.state.db, user.id, Some(limit))
        .await
        .unwrap();
    let body = error(
        &upload(&mut api, "", png_image(64, 64)).await,
        StatusCode::PAYLOAD_TOO_LARGE,
    );
    assert_eq!(body.error, "quota_exceeded");
    assert_eq!(body.used_bytes, Some(used));
    let list: ScreenshotList =
        serde_json::from_str(&api.get("/api/v1/screenshots").await.body).unwrap();
    assert_eq!(list.screenshots.len(), 2);

    // Deleting frees space immediately.
    let response = api
        .send_raw(
            "DELETE",
            &format!("/api/v1/screenshots/{}", first.id),
            None,
            Vec::new(),
        )
        .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(me(&mut api).await.used_bytes < used / 2 + 1);
    created(&upload(&mut api, "", png_image(4, 4)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_uploads_cannot_exceed_the_quota() {
    let (server, user, mut api) = setup().await;
    let image = png_image(64, 64);
    // Measure the size of one stored upload, then allow exactly one more.
    let info = created(&upload(&mut api, "", image.clone()).await);
    let one = me(&mut api).await.used_bytes;
    let delete = format!("/api/v1/screenshots/{}", info.id);
    api.send_raw("DELETE", &delete, None, Vec::new()).await;
    users::set_quota(&server.state.db, user.id, Some(i64::try_from(one).unwrap()))
        .await
        .unwrap();

    let mut tasks = Vec::new();
    for _ in 0..4 {
        let mut client = server.api_client(&user).await;
        let image = image.clone();
        tasks.push(tokio::spawn(async move {
            upload(&mut client, "", image).await.status
        }));
    }
    let mut statuses = Vec::new();
    for task in tasks {
        statuses.push(task.await.unwrap());
    }
    let ok = statuses
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    assert_eq!(ok, 1, "{statuses:?}");
    assert!(
        statuses
            .iter()
            .all(|s| *s == StatusCode::CREATED || *s == StatusCode::PAYLOAD_TOO_LARGE)
    );
    assert_eq!(me(&mut api).await.used_bytes, one);
    assert_eq!(stored_files(&server), 2);
}

#[tokio::test]
async fn screenshots_of_other_users_cannot_be_deleted() {
    let (server, _, mut alice) = setup().await;
    let bob_user = server.create_user("bob", Role::User).await;
    let mut bob = server.api_client(&bob_user).await;
    let info = created(&upload(&mut alice, "?visibility=public", png_image(4, 4)).await);
    let path = format!("/api/v1/screenshots/{}", info.id);

    let body = error(
        &bob.send_raw("DELETE", &path, None, Vec::new()).await,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(body.error, "not_found");
    // Bob does not see it either.
    assert!(!bob.get("/api/v1/screenshots").await.body.contains(&info.id));
    assert_eq!(stored_files(&server), 2);

    let response = alice.send_raw("DELETE", &path, None, Vec::new()).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(stored_files(&server), 0);
    error(
        &alice.send_raw("DELETE", &path, None, Vec::new()).await,
        StatusCode::NOT_FOUND,
    );
    error(
        &alice
            .send_raw("DELETE", "/api/v1/screenshots/not-a-uuid", None, Vec::new())
            .await,
        StatusCode::NOT_FOUND,
    );
}

#[tokio::test]
async fn list_is_newest_first_and_validates_the_limit() {
    let (server, _, mut api) = setup().await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(created(&upload(&mut api, "", png_image(4, 4)).await).id);
        server.clock.advance(10);
    }
    let list: ScreenshotList =
        serde_json::from_str(&api.get("/api/v1/screenshots?limit=2").await.body).unwrap();
    let got: Vec<_> = list.screenshots.into_iter().map(|s| s.id).collect();
    assert_eq!(got, [ids[2].clone(), ids[1].clone()]);

    for bad in ["0", "101", "x"] {
        let response = api.get(&format!("/api/v1/screenshots?limit={bad}")).await;
        assert_eq!(
            error(&response, StatusCode::BAD_REQUEST).error,
            "invalid_request"
        );
    }
}

#[tokio::test]
async fn upload_endpoints_need_a_token() {
    let (server, _, _) = setup().await;
    let mut anonymous = server.client("198.51.100.1");
    let response = upload(&mut anonymous, "", png_image(4, 4)).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        anonymous.get("/api/v1/screenshots").await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(stored_files(&server), 0);
}

#[tokio::test]
async fn unlimited_server_default_quota() {
    let server = TestServer::with_env(&[("SCRN_DEFAULT_QUOTA_MB", "0")]).await;
    let user = server.create_user("alice", Role::User).await;
    let mut api = server.api_client(&user).await;
    assert_eq!(me(&mut api).await.quota_bytes, None);
}
