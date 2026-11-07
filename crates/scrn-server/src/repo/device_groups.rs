//! The `device_groups` table: groups of API tokens, such as "Work" or
//! "Home". Every user has one default group, created by a database trigger
//! together with the user; it cannot be deleted.

use sqlx::{FromRow, SqliteConnection, SqliteExecutor};

use super::RepoError;

/// A group of a user with the number of tokens in it.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct DeviceGroup {
    pub id: i64,
    pub user_id: i64,
    /// `None` for a default group that was never renamed.
    pub name: Option<String>,
    pub is_default: bool,
    pub created_at: i64,
    /// Tokens in the group, revoked ones included.
    pub tokens: i64,
}

/// Selected columns, shared by all queries of this module.
macro_rules! columns {
    () => {
        "g.id, g.user_id, g.name, g.is_default, g.created_at, \
    (SELECT COUNT(*) FROM api_tokens t WHERE t.group_id = g.id) AS tokens"
    };
}

/// The groups of a user: the default group first, then by name.
pub async fn list_for_user(
    db: impl SqliteExecutor<'_>,
    user_id: i64,
) -> Result<Vec<DeviceGroup>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM device_groups g WHERE g.user_id = ? \
         ORDER BY g.is_default DESC, g.name COLLATE NOCASE, g.id"
    );
    Ok(sqlx::query_as(sql).bind(user_id).fetch_all(db).await?)
}

/// A group, only if it belongs to `user_id`.
pub async fn get_for_user(
    db: impl SqliteExecutor<'_>,
    id: i64,
    user_id: i64,
) -> Result<Option<DeviceGroup>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM device_groups g WHERE g.id = ? AND g.user_id = ?"
    );
    Ok(sqlx::query_as(sql)
        .bind(id)
        .bind(user_id)
        .fetch_optional(db)
        .await?)
}

/// Creates a group. Returns [`RepoError::Conflict`] if the user already has
/// a group of that name (compared case-insensitively).
pub async fn create(
    db: impl SqliteExecutor<'_>,
    user_id: i64,
    name: &str,
    now: i64,
) -> Result<i64, RepoError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO device_groups (user_id, name, created_at) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(now)
    .fetch_one(db)
    .await?)
}

/// Renames a group of `user_id`. Returns false if it does not exist or
/// belongs to someone else, [`RepoError::Conflict`] if the name is taken.
pub async fn rename(
    db: impl SqliteExecutor<'_>,
    id: i64,
    user_id: i64,
    name: &str,
) -> Result<bool, RepoError> {
    let result = sqlx::query("UPDATE device_groups SET name = ? WHERE id = ? AND user_id = ?")
        .bind(name)
        .bind(id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Deletes a group of `user_id` that is not the default group. Its tokens,
/// and with them their screenshots, move to the default group. Returns
/// false if there is no such group. Run it in a transaction.
pub async fn delete(db: &mut SqliteConnection, id: i64, user_id: i64) -> Result<bool, RepoError> {
    sqlx::query(
        "UPDATE api_tokens SET group_id = \
         (SELECT id FROM device_groups WHERE user_id = ? AND is_default = 1) \
         WHERE group_id = ? AND user_id = ? \
         AND EXISTS (SELECT 1 FROM device_groups WHERE id = ? AND user_id = ? AND is_default = 0)",
    )
    .bind(user_id)
    .bind(id)
    .bind(user_id)
    .bind(id)
    .bind(user_id)
    .execute(&mut *db)
    .await?;
    let deleted =
        sqlx::query("DELETE FROM device_groups WHERE id = ? AND user_id = ? AND is_default = 0")
            .bind(id)
            .bind(user_id)
            .execute(&mut *db)
            .await?;
    Ok(deleted.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{api_tokens, test_util};

    #[tokio::test]
    async fn every_user_starts_with_a_default_group() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let groups = list_for_user(&pool, alice.id).await.unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].is_default);
        assert_eq!(groups[0].name, None);
        assert_eq!(groups[0].tokens, 0);
    }

    #[tokio::test]
    async fn create_rename_and_list() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        let work = create(&pool, alice.id, "Work", 10).await.unwrap();
        create(&pool, alice.id, "Home", 11).await.unwrap();
        assert!(matches!(
            create(&pool, alice.id, "WORK", 12).await,
            Err(RepoError::Conflict)
        ));
        // Names are per user.
        create(&pool, bob.id, "Work", 12).await.unwrap();

        let names: Vec<_> = list_for_user(&pool, alice.id)
            .await
            .unwrap()
            .into_iter()
            .map(|g| g.name)
            .collect();
        assert_eq!(names, [None, Some("Home".into()), Some("Work".into())]);

        assert!(rename(&pool, work, alice.id, "Office").await.unwrap());
        assert!(!rename(&pool, work, bob.id, "Mine").await.unwrap());
        assert!(matches!(
            rename(&pool, work, alice.id, "home").await,
            Err(RepoError::Conflict)
        ));
        let group = get_for_user(&pool, work, alice.id).await.unwrap().unwrap();
        assert_eq!(group.name.as_deref(), Some("Office"));
        assert_eq!(get_for_user(&pool, work, bob.id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn deleting_moves_tokens_to_the_default_group() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        let default = list_for_user(&pool, alice.id).await.unwrap()[0].id;
        let work = create(&pool, alice.id, "Work", 10).await.unwrap();
        let token = api_tokens::create(
            &pool,
            alice.id,
            Some(work),
            "laptop",
            &test_util::hash(1),
            20,
        )
        .await
        .unwrap();
        assert_eq!(token.group_id, work);

        let mut conn = pool.acquire().await.unwrap();
        assert!(!delete(&mut conn, work, bob.id).await.unwrap());
        assert!(!delete(&mut conn, default, alice.id).await.unwrap());
        assert!(delete(&mut conn, work, alice.id).await.unwrap());
        drop(conn);

        let tokens = api_tokens::list_for_user(&pool, alice.id).await.unwrap();
        assert_eq!(tokens[0].group_id, default);
        let groups = list_for_user(&pool, alice.id).await.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].tokens, 1);
    }
}
