//! The `api_tokens` table: bearer tokens for the desktop client. Only the
//! SHA-256 hash of a token is stored.

use sqlx::{FromRow, SqliteExecutor};

use super::{RepoError, Sha256};

/// A row of the `api_tokens` table (without the token hash).
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct ApiToken {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Selected columns, shared by all queries of this module.
macro_rules! columns {
    () => {
        "id, user_id, name, created_at, last_used_at, revoked_at"
    };
}

pub async fn create(
    db: impl SqliteExecutor<'_>,
    user_id: i64,
    name: &str,
    token_hash: &Sha256,
    now: i64,
) -> Result<ApiToken, RepoError> {
    let sql = concat!(
        "INSERT INTO api_tokens (user_id, name, token_hash, created_at) \
         VALUES (?, ?, ?, ?) RETURNING ",
        columns!()
    );
    Ok(sqlx::query_as(sql)
        .bind(user_id)
        .bind(name)
        .bind(&token_hash[..])
        .bind(now)
        .fetch_one(db)
        .await?)
}

/// Finds a token that has not been revoked by its hash.
pub async fn get_active_by_hash(
    db: impl SqliteExecutor<'_>,
    token_hash: &Sha256,
) -> Result<Option<ApiToken>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM api_tokens WHERE token_hash = ? AND revoked_at IS NULL"
    );
    Ok(sqlx::query_as(sql)
        .bind(&token_hash[..])
        .fetch_optional(db)
        .await?)
}

/// All tokens of a user including revoked ones, newest first.
pub async fn list_for_user(
    db: impl SqliteExecutor<'_>,
    user_id: i64,
) -> Result<Vec<ApiToken>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM api_tokens WHERE user_id = ? ORDER BY created_at DESC, id DESC"
    );
    Ok(sqlx::query_as(sql).bind(user_id).fetch_all(db).await?)
}

/// Revokes a token of `user_id`. Returns false if the token does not exist,
/// belongs to someone else or is already revoked.
pub async fn revoke(
    db: impl SqliteExecutor<'_>,
    id: i64,
    user_id: i64,
    now: i64,
) -> Result<bool, RepoError> {
    let result = sqlx::query(
        "UPDATE api_tokens SET revoked_at = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
    )
    .bind(now)
    .bind(id)
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Updates `last_used_at`.
pub async fn touch_last_used(
    db: impl SqliteExecutor<'_>,
    id: i64,
    now: i64,
) -> Result<bool, RepoError> {
    let result = sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE id = ?")
        .bind(now)
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
    async fn create_find_and_touch() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let token = create(&pool, user.id, "laptop", &test_util::hash(1), 100)
            .await
            .unwrap();
        assert_eq!(token.name, "laptop");
        assert_eq!(token.last_used_at, None);

        assert_eq!(
            get_active_by_hash(&pool, &test_util::hash(1))
                .await
                .unwrap(),
            Some(token.clone())
        );
        assert_eq!(
            get_active_by_hash(&pool, &test_util::hash(2))
                .await
                .unwrap(),
            None
        );

        assert!(touch_last_used(&pool, token.id, 150).await.unwrap());
        let listed = list_for_user(&pool, user.id).await.unwrap();
        assert_eq!(listed[0].last_used_at, Some(150));
    }

    #[tokio::test]
    async fn revoked_token_is_not_active() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let token = create(&pool, user.id, "laptop", &test_util::hash(1), 100)
            .await
            .unwrap();
        assert!(revoke(&pool, token.id, user.id, 200).await.unwrap());
        assert!(!revoke(&pool, token.id, user.id, 201).await.unwrap());
        assert_eq!(
            get_active_by_hash(&pool, &test_util::hash(1))
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            list_for_user(&pool, user.id).await.unwrap()[0].revoked_at,
            Some(200)
        );
    }

    #[tokio::test]
    async fn other_user_cannot_revoke() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        let token = create(&pool, alice.id, "laptop", &test_util::hash(1), 100)
            .await
            .unwrap();
        assert!(!revoke(&pool, token.id, bob.id, 200).await.unwrap());
        assert!(
            get_active_by_hash(&pool, &test_util::hash(1))
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn list_is_per_user_and_newest_first() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        create(&pool, alice.id, "old", &test_util::hash(1), 100)
            .await
            .unwrap();
        create(&pool, alice.id, "new", &test_util::hash(2), 200)
            .await
            .unwrap();
        create(&pool, bob.id, "bob", &test_util::hash(3), 300)
            .await
            .unwrap();
        let names: Vec<_> = list_for_user(&pool, alice.id)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, ["new", "old"]);
    }

    #[tokio::test]
    async fn constraints() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        create(&pool, user.id, "a", &test_util::hash(1), 100)
            .await
            .unwrap();
        assert!(matches!(
            create(&pool, user.id, "b", &test_util::hash(1), 100).await,
            Err(RepoError::Conflict)
        ));
        assert!(matches!(
            create(&pool, user.id, "", &test_util::hash(2), 100).await,
            Err(RepoError::Db(_))
        ));
        assert!(matches!(
            create(&pool, 999, "ghost", &test_util::hash(3), 100).await,
            Err(RepoError::Db(_))
        ));
    }

    #[tokio::test]
    async fn deleting_the_user_cascades() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        create(&pool, user.id, "laptop", &test_util::hash(1), 100)
            .await
            .unwrap();
        users::delete(&pool, user.id).await.unwrap();
        assert_eq!(
            get_active_by_hash(&pool, &test_util::hash(1))
                .await
                .unwrap(),
            None
        );
    }
}
