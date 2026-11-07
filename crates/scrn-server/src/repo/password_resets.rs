//! The `password_resets` table: single-use links that set a new password.

use sqlx::{FromRow, SqliteExecutor};

use super::{RepoError, Sha256};

/// A row of the `password_resets` table (without the token hash).
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct PasswordReset {
    pub id: i64,
    pub user_id: i64,
    pub created_by: Option<i64>,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_at: Option<i64>,
}

/// Selected columns, shared by all queries of this module.
macro_rules! columns {
    () => {
        "id, user_id, created_by, created_at, expires_at, used_at"
    };
}

/// Creates a reset link for `user_id`. Older links of the same user,
/// used or not, are removed, so only the newest one works.
pub async fn replace(
    db: &mut sqlx::SqliteConnection,
    token_hash: &Sha256,
    user_id: i64,
    created_by: Option<i64>,
    now: i64,
    expires_at: i64,
) -> Result<PasswordReset, RepoError> {
    sqlx::query("DELETE FROM password_resets WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *db)
        .await?;
    let sql = concat!(
        "INSERT INTO password_resets (token_hash, user_id, created_by, created_at, expires_at) \
         VALUES (?, ?, ?, ?, ?) RETURNING ",
        columns!()
    );
    Ok(sqlx::query_as(sql)
        .bind(&token_hash[..])
        .bind(user_id)
        .bind(created_by)
        .bind(now)
        .bind(expires_at)
        .fetch_one(&mut *db)
        .await?)
}

/// Finds an unused, unexpired reset link by the hash of its token.
pub async fn get_usable_by_token_hash(
    db: impl SqliteExecutor<'_>,
    token_hash: &Sha256,
    now: i64,
) -> Result<Option<PasswordReset>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM password_resets \
         WHERE token_hash = ? AND used_at IS NULL AND expires_at > ?"
    );
    Ok(sqlx::query_as(sql)
        .bind(&token_hash[..])
        .bind(now)
        .fetch_optional(db)
        .await?)
}

/// Marks a reset link as used. Only succeeds while it is still unused and
/// unexpired, so a link cannot be used twice. Returns false otherwise.
pub async fn mark_used(db: impl SqliteExecutor<'_>, id: i64, now: i64) -> Result<bool, RepoError> {
    let result = sqlx::query(
        "UPDATE password_resets SET used_at = ? \
         WHERE id = ? AND used_at IS NULL AND expires_at > ?",
    )
    .bind(now)
    .bind(id)
    .bind(now)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{test_util, users};

    #[tokio::test]
    async fn create_find_and_use_once() {
        let (_dir, pool) = test_util::pool().await;
        let admin = test_util::user(&pool, "admin").await;
        let bob = test_util::user(&pool, "bob").await;
        let mut conn = pool.acquire().await.unwrap();
        let reset = replace(
            &mut conn,
            &test_util::hash(1),
            bob.id,
            Some(admin.id),
            100,
            200,
        )
        .await
        .unwrap();
        assert_eq!((reset.user_id, reset.created_by), (bob.id, Some(admin.id)));

        let found = get_usable_by_token_hash(&pool, &test_util::hash(1), 150)
            .await
            .unwrap();
        assert_eq!(found, Some(reset.clone()));
        assert_eq!(
            get_usable_by_token_hash(&pool, &test_util::hash(1), 200)
                .await
                .unwrap(),
            None
        );
        assert!(mark_used(&pool, reset.id, 150).await.unwrap());
        assert!(!mark_used(&pool, reset.id, 151).await.unwrap());
        assert_eq!(
            get_usable_by_token_hash(&pool, &test_util::hash(1), 152)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn expired_link_cannot_be_used() {
        let (_dir, pool) = test_util::pool().await;
        let bob = test_util::user(&pool, "bob").await;
        let mut conn = pool.acquire().await.unwrap();
        let reset = replace(&mut conn, &test_util::hash(1), bob.id, None, 100, 200)
            .await
            .unwrap();
        assert!(!mark_used(&pool, reset.id, 200).await.unwrap());
    }

    #[tokio::test]
    async fn a_new_link_replaces_the_old_one() {
        let (_dir, pool) = test_util::pool().await;
        let bob = test_util::user(&pool, "bob").await;
        let carol = test_util::user(&pool, "carol").await;
        let mut conn = pool.acquire().await.unwrap();
        replace(&mut conn, &test_util::hash(1), bob.id, None, 100, 200)
            .await
            .unwrap();
        replace(&mut conn, &test_util::hash(3), carol.id, None, 100, 200)
            .await
            .unwrap();
        replace(&mut conn, &test_util::hash(2), bob.id, None, 110, 210)
            .await
            .unwrap();
        let (old, new, other) = (test_util::hash(1), test_util::hash(2), test_util::hash(3));
        let usable = |hash| get_usable_by_token_hash(&pool, hash, 150);
        assert_eq!(usable(&old).await.unwrap(), None);
        assert!(usable(&new).await.unwrap().is_some());
        assert!(
            usable(&other).await.unwrap().is_some(),
            "other users keep theirs"
        );
    }

    #[tokio::test]
    async fn deleting_users_cascades_or_clears_the_creator() {
        let (_dir, pool) = test_util::pool().await;
        let admin = test_util::user(&pool, "admin").await;
        let bob = test_util::user(&pool, "bob").await;
        let mut conn = pool.acquire().await.unwrap();
        replace(
            &mut conn,
            &test_util::hash(1),
            bob.id,
            Some(admin.id),
            100,
            200,
        )
        .await
        .unwrap();
        drop(conn);
        users::delete(&pool, admin.id).await.unwrap();
        let reset = get_usable_by_token_hash(&pool, &test_util::hash(1), 150)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reset.created_by, None);
        users::delete(&pool, bob.id).await.unwrap();
        assert_eq!(
            get_usable_by_token_hash(&pool, &test_util::hash(1), 150)
                .await
                .unwrap(),
            None
        );
    }
}
