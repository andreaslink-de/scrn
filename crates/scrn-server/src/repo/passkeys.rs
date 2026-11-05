//! The `passkeys` table: WebAuthn credentials. The credential itself is
//! stored as serialized by the WebAuthn library (added in step S9).

use sqlx::{FromRow, SqliteExecutor};

use super::RepoError;

/// A row of the `passkeys` table.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Passkey {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub credential_id: Vec<u8>,
    pub credential: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

/// Values for a new passkey.
#[derive(Debug, Clone)]
pub struct NewPasskey<'a> {
    pub user_id: i64,
    pub name: &'a str,
    pub credential_id: &'a [u8],
    pub credential: &'a str,
}

/// Selected columns, shared by all queries of this module.
macro_rules! columns {
    () => {
        "id, user_id, name, credential_id, credential, created_at, last_used_at"
    };
}

/// Stores a passkey. Returns [`RepoError::Conflict`] if the credential ID
/// is already registered.
pub async fn create(
    db: impl SqliteExecutor<'_>,
    new: &NewPasskey<'_>,
    now: i64,
) -> Result<Passkey, RepoError> {
    let sql = concat!(
        "INSERT INTO passkeys (user_id, name, credential_id, credential, created_at) \
         VALUES (?, ?, ?, ?, ?) RETURNING ",
        columns!()
    );
    Ok(sqlx::query_as(sql)
        .bind(new.user_id)
        .bind(new.name)
        .bind(new.credential_id)
        .bind(new.credential)
        .bind(now)
        .fetch_one(db)
        .await?)
}

/// All passkeys of a user, oldest first.
pub async fn list_for_user(
    db: impl SqliteExecutor<'_>,
    user_id: i64,
) -> Result<Vec<Passkey>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM passkeys WHERE user_id = ? ORDER BY created_at, id"
    );
    Ok(sqlx::query_as(sql).bind(user_id).fetch_all(db).await?)
}

/// Number of passkeys of a user, used to protect the last login method.
pub async fn count_for_user(db: impl SqliteExecutor<'_>, user_id: i64) -> Result<i64, RepoError> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM passkeys WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(db)
            .await?,
    )
}

pub async fn get_by_credential_id(
    db: impl SqliteExecutor<'_>,
    credential_id: &[u8],
) -> Result<Option<Passkey>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM passkeys WHERE credential_id = ?"
    );
    Ok(sqlx::query_as(sql)
        .bind(credential_id)
        .fetch_optional(db)
        .await?)
}

/// Stores the updated credential (e.g. the signature counter) after a login.
pub async fn record_use(
    db: impl SqliteExecutor<'_>,
    id: i64,
    credential: &str,
    now: i64,
) -> Result<bool, RepoError> {
    let result = sqlx::query("UPDATE passkeys SET credential = ?, last_used_at = ? WHERE id = ?")
        .bind(credential)
        .bind(now)
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Renames a passkey of `user_id`. Returns false if it does not exist or
/// belongs to someone else.
pub async fn rename(
    db: impl SqliteExecutor<'_>,
    id: i64,
    user_id: i64,
    name: &str,
) -> Result<bool, RepoError> {
    let result = sqlx::query("UPDATE passkeys SET name = ? WHERE id = ? AND user_id = ?")
        .bind(name)
        .bind(id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Deletes a passkey of `user_id`. Returns false if it does not exist or
/// belongs to someone else.
pub async fn delete(db: impl SqliteExecutor<'_>, id: i64, user_id: i64) -> Result<bool, RepoError> {
    let result = sqlx::query("DELETE FROM passkeys WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{test_util, users};

    fn new<'a>(user_id: i64, name: &'a str, credential_id: &'a [u8]) -> NewPasskey<'a> {
        NewPasskey {
            user_id,
            name,
            credential_id,
            credential: "{}",
        }
    }

    #[tokio::test]
    async fn create_list_and_find() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let first = create(&pool, &new(user.id, "phone", b"cred-1"), 100)
            .await
            .unwrap();
        let second = create(&pool, &new(user.id, "laptop", b"cred-2"), 200)
            .await
            .unwrap();

        assert_eq!(
            list_for_user(&pool, user.id).await.unwrap(),
            [first.clone(), second]
        );
        assert_eq!(count_for_user(&pool, user.id).await.unwrap(), 2);
        assert_eq!(
            get_by_credential_id(&pool, b"cred-1").await.unwrap(),
            Some(first)
        );
        assert_eq!(get_by_credential_id(&pool, b"nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn credential_ids_are_unique() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        create(&pool, &new(alice.id, "phone", b"cred-1"), 100)
            .await
            .unwrap();
        assert!(matches!(
            create(&pool, &new(bob.id, "phone", b"cred-1"), 100).await,
            Err(RepoError::Conflict)
        ));
    }

    #[tokio::test]
    async fn record_use_rename_and_delete() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        let key = create(&pool, &new(alice.id, "phone", b"cred-1"), 100)
            .await
            .unwrap();

        assert!(
            record_use(&pool, key.id, "{\"counter\":1}", 150)
                .await
                .unwrap()
        );
        assert!(!rename(&pool, key.id, bob.id, "stolen").await.unwrap());
        assert!(rename(&pool, key.id, alice.id, "work phone").await.unwrap());
        let stored = get_by_credential_id(&pool, b"cred-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.name, "work phone");
        assert_eq!(stored.credential, "{\"counter\":1}");
        assert_eq!(stored.last_used_at, Some(150));

        assert!(!delete(&pool, key.id, bob.id).await.unwrap());
        assert!(delete(&pool, key.id, alice.id).await.unwrap());
        assert_eq!(count_for_user(&pool, alice.id).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn deleting_the_user_cascades() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        create(&pool, &new(user.id, "phone", b"cred-1"), 100)
            .await
            .unwrap();
        users::delete(&pool, user.id).await.unwrap();
        assert_eq!(get_by_credential_id(&pool, b"cred-1").await.unwrap(), None);
    }
}
