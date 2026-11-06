//! SQLite connection pool and migrations.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// Embedded migrations from `crates/scrn-server/migrations`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Errors while opening or migrating the database.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("failed to open the database: {0}")]
    Open(#[source] sqlx::Error),
    #[error("failed to run database migrations: {0}")]
    Migrate(#[source] sqlx::migrate::MigrateError),
}

/// Opens (and creates if needed) the SQLite database at `path` in WAL mode
/// with foreign keys enabled, then applies all pending migrations.
pub async fn connect(path: &Path) -> Result<SqlitePool, DbError> {
    let options = SqliteConnectOptions::from_str("sqlite:")
        .map_err(DbError::Open)?
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .map_err(DbError::Open)?;

    MIGRATOR.run(&pool).await.map_err(DbError::Migrate)?;
    Ok(pool)
}

/// Writes a consistent copy of the database to `target` while the server
/// keeps running (`VACUUM INTO`). The target must not exist yet.
pub async fn backup(pool: &SqlitePool, target: &Path) -> Result<(), BackupError> {
    if target.exists() {
        return Err(BackupError::Exists(target.display().to_string()));
    }
    let target = target.to_str().ok_or(BackupError::Path)?.to_owned();
    sqlx::query("VACUUM INTO ?")
        .bind(target)
        .execute(pool)
        .await
        .map_err(BackupError::Db)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("backup target {0} already exists")]
    Exists(String),
    #[error("backup path is not valid UTF-8")]
    Path,
    #[error("backup failed: {0}")]
    Db(#[source] sqlx::Error),
}

/// Runs a trivial query to check that the database answers.
pub async fn ping(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT 1").execute(pool).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_database_in_wal_mode_with_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scrn.db");
        let pool = connect(&path).await.unwrap();
        assert!(path.exists());

        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");

        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);

        ping(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn backup_is_a_complete_database() {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(&dir.path().join("scrn.db")).await.unwrap();
        sqlx::query("INSERT INTO users (username, created_at) VALUES ('alice', 1)")
            .execute(&pool)
            .await
            .unwrap();
        let target = dir.path().join("backup.db");
        backup(&pool, &target).await.unwrap();
        assert!(matches!(
            backup(&pool, &target).await,
            Err(BackupError::Exists(_))
        ));

        let copy = connect(&target).await.unwrap();
        let name: String = sqlx::query_scalar("SELECT username FROM users")
            .fetch_one(&copy)
            .await
            .unwrap();
        assert_eq!(name, "alice");
    }

    #[tokio::test]
    async fn reopening_keeps_data_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scrn.db");

        let pool = connect(&path).await.unwrap();
        sqlx::query("CREATE TABLE probe (v INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO probe (v) VALUES (42)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let pool = connect(&path).await.unwrap();
        let v: i64 = sqlx::query_scalar("SELECT v FROM probe")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(v, 42);
    }
}
