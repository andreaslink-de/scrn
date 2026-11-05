//! The `invites` table: single-use registration links created by an admin.

use sqlx::{FromRow, SqliteExecutor};

use super::{RepoError, Sha256};

/// A row of the `invites` table (without the token hash).
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Invite {
    pub id: i64,
    pub created_by: i64,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_by: Option<i64>,
    pub used_at: Option<i64>,
}

impl Invite {
    /// True if the invite is unused and has not expired at `now`.
    pub fn is_usable(&self, now: i64) -> bool {
        self.used_at.is_none() && self.expires_at > now
    }
}

/// Selected columns, shared by all queries of this module.
macro_rules! columns {
    () => {
        "id, created_by, created_at, expires_at, used_by, used_at"
    };
}

pub async fn create(
    db: impl SqliteExecutor<'_>,
    token_hash: &Sha256,
    created_by: i64,
    now: i64,
    expires_at: i64,
) -> Result<Invite, RepoError> {
    let sql = concat!(
        "INSERT INTO invites (token_hash, created_by, created_at, expires_at) \
         VALUES (?, ?, ?, ?) RETURNING ",
        columns!()
    );
    Ok(sqlx::query_as(sql)
        .bind(&token_hash[..])
        .bind(created_by)
        .bind(now)
        .bind(expires_at)
        .fetch_one(db)
        .await?)
}

/// Finds an unused, unexpired invite by the hash of its token.
pub async fn get_usable_by_token_hash(
    db: impl SqliteExecutor<'_>,
    token_hash: &Sha256,
    now: i64,
) -> Result<Option<Invite>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM invites \
         WHERE token_hash = ? AND used_at IS NULL AND expires_at > ?"
    );
    Ok(sqlx::query_as(sql)
        .bind(&token_hash[..])
        .bind(now)
        .fetch_optional(db)
        .await?)
}

/// Marks an invite as used by `used_by`. The update only succeeds if the
/// invite is still unused and unexpired, so two concurrent registrations
/// cannot both consume it. Returns false if it was not usable.
pub async fn mark_used(
    db: impl SqliteExecutor<'_>,
    id: i64,
    used_by: i64,
    now: i64,
) -> Result<bool, RepoError> {
    let result = sqlx::query(
        "UPDATE invites SET used_by = ?, used_at = ? \
         WHERE id = ? AND used_at IS NULL AND expires_at > ?",
    )
    .bind(used_by)
    .bind(now)
    .bind(id)
    .bind(now)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// All invites, newest first.
pub async fn list(db: impl SqliteExecutor<'_>) -> Result<Vec<Invite>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM invites ORDER BY created_at DESC, id DESC"
    );
    Ok(sqlx::query_as(sql).fetch_all(db).await?)
}

/// Deletes an unused invite (revocation). Used invites stay as a record.
/// Returns false if the invite does not exist or was already used.
pub async fn delete_unused(db: impl SqliteExecutor<'_>, id: i64) -> Result<bool, RepoError> {
    let result = sqlx::query("DELETE FROM invites WHERE id = ? AND used_at IS NULL")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{test_util, users};

    #[tokio::test]
    async fn create_and_find_usable() {
        let (_dir, pool) = test_util::pool().await;
        let admin = test_util::user(&pool, "admin").await;
        let invite = create(&pool, &test_util::hash(1), admin.id, 100, 200)
            .await
            .unwrap();
        assert_eq!(invite.created_by, admin.id);
        assert!(invite.is_usable(150));
        assert!(!invite.is_usable(200));

        let found = get_usable_by_token_hash(&pool, &test_util::hash(1), 150)
            .await
            .unwrap();
        assert_eq!(found, Some(invite));
        assert_eq!(
            get_usable_by_token_hash(&pool, &test_util::hash(2), 150)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn expired_invite_is_not_usable() {
        let (_dir, pool) = test_util::pool().await;
        let admin = test_util::user(&pool, "admin").await;
        let invite = create(&pool, &test_util::hash(1), admin.id, 100, 200)
            .await
            .unwrap();
        let bob = test_util::user(&pool, "bob").await;
        assert_eq!(
            get_usable_by_token_hash(&pool, &test_util::hash(1), 200)
                .await
                .unwrap(),
            None
        );
        assert!(!mark_used(&pool, invite.id, bob.id, 200).await.unwrap());
    }

    #[tokio::test]
    async fn invite_can_be_used_only_once() {
        let (_dir, pool) = test_util::pool().await;
        let admin = test_util::user(&pool, "admin").await;
        let bob = test_util::user(&pool, "bob").await;
        let carol = test_util::user(&pool, "carol").await;
        let invite = create(&pool, &test_util::hash(1), admin.id, 100, 200)
            .await
            .unwrap();

        assert!(mark_used(&pool, invite.id, bob.id, 150).await.unwrap());
        assert!(!mark_used(&pool, invite.id, carol.id, 151).await.unwrap());
        assert_eq!(
            get_usable_by_token_hash(&pool, &test_util::hash(1), 152)
                .await
                .unwrap(),
            None
        );

        let stored = &list(&pool).await.unwrap()[0];
        assert_eq!(stored.used_by, Some(bob.id));
        assert_eq!(stored.used_at, Some(150));
        assert!(!delete_unused(&pool, invite.id).await.unwrap());
    }

    #[tokio::test]
    async fn token_hashes_are_unique() {
        let (_dir, pool) = test_util::pool().await;
        let admin = test_util::user(&pool, "admin").await;
        create(&pool, &test_util::hash(1), admin.id, 100, 200)
            .await
            .unwrap();
        let err = create(&pool, &test_util::hash(1), admin.id, 100, 200)
            .await
            .unwrap_err();
        assert!(matches!(err, RepoError::Conflict));
    }

    #[tokio::test]
    async fn used_by_and_used_at_must_be_set_together() {
        let (_dir, pool) = test_util::pool().await;
        let admin = test_util::user(&pool, "admin").await;
        let invite = create(&pool, &test_util::hash(1), admin.id, 100, 200)
            .await
            .unwrap();
        let result = sqlx::query("UPDATE invites SET used_at = 1 WHERE id = ?")
            .bind(invite.id)
            .execute(&pool)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_and_delete_unused() {
        let (_dir, pool) = test_util::pool().await;
        let admin = test_util::user(&pool, "admin").await;
        let first = create(&pool, &test_util::hash(1), admin.id, 100, 200)
            .await
            .unwrap();
        let second = create(&pool, &test_util::hash(2), admin.id, 101, 200)
            .await
            .unwrap();
        assert_eq!(list(&pool).await.unwrap(), [second.clone(), first.clone()]);
        assert!(delete_unused(&pool, first.id).await.unwrap());
        assert_eq!(list(&pool).await.unwrap(), [second]);
    }

    #[tokio::test]
    async fn deleting_the_creator_cascades() {
        let (_dir, pool) = test_util::pool().await;
        let admin = test_util::user(&pool, "admin").await;
        create(&pool, &test_util::hash(1), admin.id, 100, 200)
            .await
            .unwrap();
        users::delete(&pool, admin.id).await.unwrap();
        assert!(list(&pool).await.unwrap().is_empty());
    }
}
