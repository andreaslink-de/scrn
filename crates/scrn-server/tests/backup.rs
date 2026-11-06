//! Integration tests for the backup download in the administration.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::{Client, PASSWORD, TestServer, png_image, upload};
use scrn_proto::ScreenshotInfo;
use scrn_server::repo::users::{Role, User};
use scrn_server::{backup, db};
use uuid::Uuid;

struct Setup {
    server: TestServer,
    admin: User,
    browser: Client,
}

async fn setup() -> Setup {
    let server = TestServer::new().await;
    let admin = server.create_user("root", Role::Admin).await;
    let mut browser = server.client("192.0.2.1");
    browser
        .login("root", PASSWORD)
        .await
        .assert_redirect("/app");
    Setup {
        server,
        admin,
        browser,
    }
}

async fn shot(server: &TestServer, user: &User) -> Uuid {
    let mut api = server.api_client(user).await;
    let response = upload(&mut api, "", png_image(6, 4)).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.body);
    let info: ScreenshotInfo = serde_json::from_str(&response.body).unwrap();
    Uuid::parse_str(&info.id).unwrap()
}

/// Splits an archive into (name, contents) and checks the format on the way.
fn entries(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
    assert_eq!(archive.len() % 512, 0);
    let mut found = Vec::new();
    let mut at = 0;
    loop {
        let header = &archive[at..at + 512];
        if header.iter().all(|b| *b == 0) {
            // Two zero blocks end the archive, and nothing follows.
            assert!(archive[at..].iter().all(|b| *b == 0));
            assert_eq!(archive.len() - at, 1024);
            return found;
        }
        let text = |range: std::ops::Range<usize>| {
            std::str::from_utf8(&header[range])
                .unwrap()
                .trim_end_matches(['\0', ' '])
                .to_owned()
        };
        assert_eq!(&header[257..263], b"ustar\0");
        let mut copy = header.to_vec();
        copy[148..156].fill(b' ');
        let sum: u64 = copy.iter().map(|b| u64::from(*b)).sum();
        assert_eq!(u64::from_str_radix(&text(148..155), 8).unwrap(), sum);
        let size = usize::try_from(u64::from_str_radix(&text(124..136), 8).unwrap()).unwrap();
        let data = archive[at + 512..at + 512 + size].to_vec();
        found.push((text(0..100), data));
        at += 512 + size.div_ceil(512) * 512;
    }
}

fn snapshots_left(server: &TestServer) -> usize {
    std::fs::read_dir(&server.state.config.data_dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(backup::SNAPSHOT_PREFIX)
        })
        .count()
}

#[tokio::test]
async fn admin_downloads_database_and_all_files() {
    let mut s = setup().await;
    let alice = s.server.create_user("alice", Role::User).await;
    let mine = shot(&s.server, &s.admin).await;
    let hers = shot(&s.server, &alice).await;

    let response = s.browser.submit("/admin", "/admin/backup", &[]).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(response.headers["content-type"], "application/x-tar");
    assert_eq!(response.headers["cache-control"], "no-store");
    assert_eq!(
        response.headers["content-disposition"],
        "attachment; filename=\"scrn-20270115-080000.tar\""
    );
    assert_eq!(
        response.headers["content-length"],
        response.bytes.len().to_string().as_str()
    );

    let files = s.server.state.config.files_dir();
    let found = entries(&response.bytes);
    let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
    let mut expected = vec!["scrn.db".to_owned()];
    for (user, id) in [(s.admin.id, mine), (alice.id, hers)] {
        expected.push(format!("files/{user}/{id}.png"));
        expected.push(format!("files/{user}/{id}.thumb.png"));
    }
    let mut sorted_names = names[1..].to_vec();
    sorted_names.sort_unstable();
    let mut sorted_expected: Vec<&str> = expected[1..].iter().map(String::as_str).collect();
    sorted_expected.sort_unstable();
    assert_eq!(names[0], "scrn.db");
    assert_eq!(sorted_names, sorted_expected);
    for (name, data) in &found[1..] {
        let path = files.join(name.strip_prefix("files/").unwrap());
        assert_eq!(&std::fs::read(path).unwrap(), data, "{name}");
    }
    // The snapshot is a working database with both screenshots.
    let dir = tempfile::tempdir().unwrap();
    let restored = dir.path().join("scrn.db");
    std::fs::write(&restored, &found[0].1).unwrap();
    let pool = db::connect(&restored).await.unwrap();
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM screenshots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);

    // Nothing stays behind on the server, and the next backup can start.
    assert_eq!(snapshots_left(&s.server), 0);
    let again = s.browser.submit("/admin", "/admin/backup", &[]).await;
    assert_eq!(again.status, StatusCode::OK);
}

#[tokio::test]
async fn only_admins_with_a_csrf_token_get_a_backup() {
    let mut s = setup().await;
    let response = s.browser.post("/admin/backup", &[("csrf", "wrong")]).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);

    s.server.create_user("alice", Role::User).await;
    let mut user = s.server.client("192.0.2.2");
    user.login("alice", PASSWORD).await;
    let csrf = user.get("/app/settings").await.csrf();
    let response = user.post("/admin/backup", &[("csrf", &csrf)]).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert!(!response.bytes.starts_with(b"scrn.db"));

    let mut visitor = s.server.client("192.0.2.3");
    visitor
        .post("/admin/backup", &[("csrf", "x")])
        .await
        .assert_redirect("/login");
    // GET is not a way around the token.
    assert_eq!(
        s.browser.get("/admin/backup").await.status,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(snapshots_left(&s.server), 0);
}

#[tokio::test]
async fn one_backup_at_a_time() {
    let mut s = setup().await;
    let running = s
        .server
        .state
        .backup_permit
        .clone()
        .try_acquire_owned()
        .unwrap();
    let response = s.browser.submit("/admin", "/admin/backup", &[]).await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert!(
        response
            .body
            .contains("Es läuft bereits ein Backup-Download")
    );
    assert_eq!(snapshots_left(&s.server), 0);
    drop(running);
    let response = s.browser.submit("/admin", "/admin/backup", &[]).await;
    assert_eq!(response.status, StatusCode::OK);
}

#[tokio::test]
async fn admin_page_offers_the_download() {
    let mut s = setup().await;
    let page = s.browser.get("/admin").await;
    assert!(page.body.contains(r#"action="/admin/backup""#));
    assert!(page.body.contains("Backup herunterladen"));
}

#[tokio::test]
async fn stale_snapshots_are_removed_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    let stale = dir
        .path()
        .join(format!("{}abc.db", backup::SNAPSHOT_PREFIX));
    std::fs::write(&stale, b"x").unwrap();
    std::fs::write(dir.path().join("scrn.db.keep"), b"x").unwrap();
    backup::remove_stale_snapshots(dir.path()).await;
    assert!(!stale.exists());
    assert!(dir.path().join("scrn.db.keep").exists());
}
