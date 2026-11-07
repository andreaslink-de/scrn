//! Integration tests for the database schema created on startup.

// Test helpers outside #[test] functions may unwrap as well.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use scrn_server::repo::screenshots::{self, NewScreenshot, PublicLink};
use scrn_server::repo::users::{self, NewUser, Role};
use scrn_server::{Config, init_state};

fn test_config(dir: &std::path::Path) -> Config {
    let data_dir = dir.to_string_lossy().into_owned();
    Config::from_lookup(|name| match name {
        "SCRN_DATA_DIR" => Some(data_dir.clone()),
        _ => None,
    })
    .unwrap()
}

#[tokio::test]
async fn startup_creates_all_tables() {
    let dir = tempfile::tempdir().unwrap();
    let state = init_state(test_config(dir.path())).await.unwrap();

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx_%' ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .unwrap();
    assert_eq!(
        tables,
        [
            "api_tokens",
            "invites",
            "passkeys",
            "password_resets",
            "screenshots",
            "sessions",
            "users"
        ]
    );

    let applied: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(
        applied,
        i64::try_from(scrn_server::db::MIGRATOR.iter().count()).unwrap()
    );
}

#[tokio::test]
async fn data_survives_restart() {
    let dir = tempfile::tempdir().unwrap();

    let state = init_state(test_config(dir.path())).await.unwrap();
    let user = users::create(
        &state.db,
        &NewUser {
            username: "alice",
            password_hash: None,
            role: Role::Admin,
        },
        1,
    )
    .await
    .unwrap();
    let shot = screenshots::insert(
        &state.db,
        &NewScreenshot {
            id: uuid::Uuid::new_v4(),
            user_id: user.id,
            public: Some(PublicLink {
                short_id: "Abc12345".to_owned(),
                expires_at: None,
            }),
            width: 10,
            height: 10,
            bytes: 100,
            thumb_bytes: 10,
            sha256: [7; 32],
        },
        2,
    )
    .await
    .unwrap();
    state.db.close().await;

    // Second start runs the migrator again, which must be a no-op.
    let state = init_state(test_config(dir.path())).await.unwrap();
    assert_eq!(users::count(&state.db).await.unwrap(), 1);
    assert_eq!(
        screenshots::get_by_short_id(&state.db, "Abc12345")
            .await
            .unwrap(),
        Some(shot)
    );
    let violations: Vec<(String,)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&state.db)
        .await
        .unwrap();
    assert!(violations.is_empty());
}
