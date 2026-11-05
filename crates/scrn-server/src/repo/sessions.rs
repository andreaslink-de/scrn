//! The `sessions` table: web logins. Only the SHA-256 hash of the session
//! ID is stored, the ID itself lives only in the user's cookie.

use sqlx::{FromRow, SqliteExecutor};

use super::{RepoError, Sha256};

/// A row of the `sessions` table (without the ID hash).
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Session {
    pub user_id: i64,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
}

/// Selected columns, shared by all queries of this module.
macro_rules! columns {
    () => {
        "user_id, created_at, last_seen_at, expires_at"
    };
}

pub async fn create(
    db: impl SqliteExecutor<'_>,
    id_hash: &Sha256,
    user_id: i64,
    now: i64,
    expires_at: i64,
) -> Result<Session, RepoError> {
    let sql = concat!(
        "INSERT INTO sessions (id_hash, user_id, created_at, last_seen_at, expires_at) \
         VALUES (?, ?, ?, ?, ?) RETURNING ",
        columns!()
    );
    Ok(sqlx::query_as(sql)
        .bind(&id_hash[..])
        .bind(user_id)
        .bind(now)
        .bind(now)
        .bind(expires_at)
        .fetch_one(db)
        .await?)
}

/// Returns the session if it exists and has not expired at `now`.
pub async fn get_valid(
    db: impl SqliteExecutor<'_>,
    id_hash: &Sha256,
    now: i64,
) -> Result<Option<Session>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM sessions WHERE id_hash = ? AND expires_at > ?"
    );
    Ok(sqlx::query_as(sql)
        .bind(&id_hash[..])
        .bind(now)
        .fetch_optional(db)
        .await?)
}

/// Records activity and extends the expiry (sliding sessions).
pub async fn touch(
    db: impl SqliteExecutor<'_>,
    id_hash: &Sha256,
    now: i64,
    expires_at: i64,
) -> Result<bool, RepoError> {
    let result =
        sqlx::query("UPDATE sessions SET last_seen_at = ?, expires_at = ? WHERE id_hash = ?")
            .bind(now)
            .bind(expires_at)
            .bind(&id_hash[..])
            .execute(db)
            .await?;
    Ok(result.rows_affected() == 1)
}

/// Deletes one session (logout).
pub async fn delete(db: impl SqliteExecutor<'_>, id_hash: &Sha256) -> Result<bool, RepoError> {
    let result = sqlx::query("DELETE FROM sessions WHERE id_hash = ?")
        .bind(&id_hash[..])
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Deletes all sessions of a user, e.g. when the account is disabled.
pub async fn delete_for_user(db: impl SqliteExecutor<'_>, user_id: i64) -> Result<u64, RepoError> {
    let result = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

/// Deletes all sessions that expired at or before `now`.
pub async fn delete_expired(db: impl SqliteExecutor<'_>, now: i64) -> Result<u64, RepoError> {
    let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{test_util, users};

    #[tokio::test]
    async fn create_read_and_expire() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let session = create(&pool, &test_util::hash(1), user.id, 100, 200)
            .await
            .unwrap();
        assert_eq!(session.last_seen_at, 100);

        assert_eq!(
            get_valid(&pool, &test_util::hash(1), 199).await.unwrap(),
            Some(session)
        );
        assert_eq!(
            get_valid(&pool, &test_util::hash(1), 200).await.unwrap(),
            None
        );
        assert_eq!(
            get_valid(&pool, &test_util::hash(2), 100).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn touch_extends_expiry() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        create(&pool, &test_util::hash(1), user.id, 100, 200)
            .await
            .unwrap();
        assert!(touch(&pool, &test_util::hash(1), 150, 300).await.unwrap());
        let session = get_valid(&pool, &test_util::hash(1), 250)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((session.last_seen_at, session.expires_at), (150, 300));
    }

    #[tokio::test]
    async fn delete_variants() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        create(&pool, &test_util::hash(1), alice.id, 100, 200)
            .await
            .unwrap();
        create(&pool, &test_util::hash(2), alice.id, 100, 300)
            .await
            .unwrap();
        create(&pool, &test_util::hash(3), bob.id, 100, 300)
            .await
            .unwrap();

        assert_eq!(delete_expired(&pool, 200).await.unwrap(), 1);
        assert_eq!(delete_for_user(&pool, alice.id).await.unwrap(), 1);
        assert!(delete(&pool, &test_util::hash(3)).await.unwrap());
        assert!(!delete(&pool, &test_util::hash(3)).await.unwrap());
    }

    #[tokio::test]
    async fn deleting_the_user_cascades() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        create(&pool, &test_util::hash(1), user.id, 100, 200)
            .await
            .unwrap();
        users::delete(&pool, user.id).await.unwrap();
        assert_eq!(
            get_valid(&pool, &test_util::hash(1), 100).await.unwrap(),
            None
        );
    }
}
