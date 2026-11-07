//! Repository layer: typed database access, one module per table.
//!
//! All functions accept any SQLite executor, so callers can pass the pool or
//! a transaction (`&mut *tx`) where several statements must be atomic. Timestamps are Unix seconds and passed in by the
//! caller, which keeps the clock out of the database layer and makes tests
//! deterministic.

pub mod api_tokens;
pub mod device_groups;
pub mod invites;
pub mod passkeys;
pub mod password_resets;
pub mod screenshots;
pub mod sessions;
pub mod users;

/// A SHA-256 digest as stored in the `*_hash` and `sha256` columns.
pub type Sha256 = [u8; 32];

/// Errors of the repository layer.
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    /// A unique constraint was violated, e.g. a username or short ID that
    /// already exists.
    #[error("record already exists")]
    Conflict,
    /// A value in the database could not be mapped to a domain type.
    #[error("invalid data in column {column}")]
    InvalidData { column: &'static str },
    #[error("database error: {0}")]
    Db(#[source] sqlx::Error),
}

impl From<sqlx::Error> for RepoError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::Database(db) if db.is_unique_violation() => Self::Conflict,
            _ => Self::Db(err),
        }
    }
}

pub(crate) fn to_sha256(column: &'static str, bytes: Vec<u8>) -> Result<Sha256, RepoError> {
    bytes
        .try_into()
        .map_err(|_| RepoError::InvalidData { column })
}

#[cfg(test)]
pub(crate) mod test_util {
    use sqlx::SqlitePool;
    use tempfile::TempDir;

    use super::users::{self, NewUser, Role, User};

    /// A migrated database in a temporary directory. Keep the `TempDir`
    /// alive for as long as the pool is used.
    pub async fn pool() -> (TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::connect(&dir.path().join("scrn.db"))
            .await
            .unwrap();
        (dir, pool)
    }

    pub async fn user(pool: &SqlitePool, name: &str) -> User {
        users::create(
            pool,
            &NewUser {
                username: name,
                password_hash: Some("$argon2id$test"),
                role: Role::User,
            },
            1_000,
        )
        .await
        .unwrap()
    }

    pub fn hash(byte: u8) -> super::Sha256 {
        [byte; 32]
    }
}
