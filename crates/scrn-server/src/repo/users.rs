//! The `users` table.

use sqlx::{FromRow, SqliteExecutor};

use super::RepoError;

/// Account role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

/// UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum Language {
    De,
    En,
}

/// UI color theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
}

/// A row of the `users` table.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: Option<String>,
    pub role: Role,
    pub language: Language,
    pub theme: Theme,
    /// Default expiry of new public links in days, 0 means never.
    pub default_expiry_days: i64,
    /// `None` means the server default, `Some(0)` means unlimited.
    pub quota_bytes: Option<i64>,
    pub created_at: i64,
    pub disabled_at: Option<i64>,
}

impl User {
    /// The quota that applies to this user in bytes, `None` if unlimited.
    /// `default_bytes` is the server default, where 0 also means unlimited.
    pub fn effective_quota(&self, default_bytes: i64) -> Option<i64> {
        match self.quota_bytes.unwrap_or(default_bytes) {
            0 => None,
            limit => Some(limit),
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled_at.is_some()
    }
}

/// Values for a new user. Settings start with their defaults.
#[derive(Debug, Clone)]
pub struct NewUser<'a> {
    pub username: &'a str,
    pub password_hash: Option<&'a str>,
    pub role: Role,
}

/// Selected columns, shared by all queries of this module.
macro_rules! columns {
    () => {
        "id, username, password_hash, role, language, theme, \
    default_expiry_days, quota_bytes, created_at, disabled_at"
    };
}

/// Creates a user. Returns [`RepoError::Conflict`] if the username exists
/// (compared case-insensitively).
pub async fn create(
    db: impl SqliteExecutor<'_>,
    new: &NewUser<'_>,
    now: i64,
) -> Result<User, RepoError> {
    let sql = concat!(
        "INSERT INTO users (username, password_hash, role, created_at) \
         VALUES (?, ?, ?, ?) RETURNING ",
        columns!()
    );
    let user = sqlx::query_as(sql)
        .bind(new.username)
        .bind(new.password_hash)
        .bind(new.role)
        .bind(now)
        .fetch_one(db)
        .await?;
    Ok(user)
}

/// Creates the first account as admin, but only while the table is empty.
/// The check and the insert are one statement, so two concurrent setup
/// requests cannot both succeed. Returns `None` if a user already exists.
pub async fn create_first_admin(
    db: impl SqliteExecutor<'_>,
    username: &str,
    password_hash: &str,
    now: i64,
) -> Result<Option<User>, RepoError> {
    let sql = concat!(
        "INSERT INTO users (username, password_hash, role, created_at) ",
        "SELECT ?, ?, 'admin', ? WHERE NOT EXISTS (SELECT 1 FROM users) ",
        "RETURNING ",
        columns!()
    );
    Ok(sqlx::query_as(sql)
        .bind(username)
        .bind(password_hash)
        .bind(now)
        .fetch_optional(db)
        .await?)
}

pub async fn get_by_id(db: impl SqliteExecutor<'_>, id: i64) -> Result<Option<User>, RepoError> {
    let sql = concat!("SELECT ", columns!(), " FROM users WHERE id = ?");
    Ok(sqlx::query_as(sql).bind(id).fetch_optional(db).await?)
}

/// Looks up a user by name, case-insensitively.
pub async fn get_by_username(
    db: impl SqliteExecutor<'_>,
    username: &str,
) -> Result<Option<User>, RepoError> {
    let sql = concat!("SELECT ", columns!(), " FROM users WHERE username = ?");
    Ok(sqlx::query_as(sql)
        .bind(username)
        .fetch_optional(db)
        .await?)
}

/// Number of users, used to detect the first start.
pub async fn count(db: impl SqliteExecutor<'_>) -> Result<i64, RepoError> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(db)
        .await?)
}

/// All users ordered by name.
pub async fn list(db: impl SqliteExecutor<'_>) -> Result<Vec<User>, RepoError> {
    let sql = concat!("SELECT ", columns!(), " FROM users ORDER BY username");
    Ok(sqlx::query_as(sql).fetch_all(db).await?)
}

/// Disables (`Some(now)`) or re-enables (`None`) a user. Returns false if
/// the user does not exist.
pub async fn set_disabled(
    db: impl SqliteExecutor<'_>,
    id: i64,
    disabled_at: Option<i64>,
) -> Result<bool, RepoError> {
    let result = sqlx::query("UPDATE users SET disabled_at = ? WHERE id = ?")
        .bind(disabled_at)
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Stores the UI preferences of a user.
pub async fn set_preferences(
    db: impl SqliteExecutor<'_>,
    id: i64,
    language: Language,
    theme: Theme,
    default_expiry_days: i64,
) -> Result<bool, RepoError> {
    let result = sqlx::query(
        "UPDATE users SET language = ?, theme = ?, default_expiry_days = ? WHERE id = ?",
    )
    .bind(language)
    .bind(theme)
    .bind(default_expiry_days)
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Sets the storage quota of a user: `None` for the server default,
/// `Some(0)` for unlimited. Existing screenshots are never touched; a quota
/// below the current usage only blocks new uploads. Returns false if the
/// user does not exist.
pub async fn set_quota(
    db: impl SqliteExecutor<'_>,
    id: i64,
    quota_bytes: Option<i64>,
) -> Result<bool, RepoError> {
    let result = sqlx::query("UPDATE users SET quota_bytes = ? WHERE id = ?")
        .bind(quota_bytes)
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Deletes a user and, through the foreign keys, all of their data.
/// Returns false if the user does not exist.
pub async fn delete(db: impl SqliteExecutor<'_>, id: i64) -> Result<bool, RepoError> {
    let result = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::test_util;

    #[tokio::test]
    async fn preferences_are_stored() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        assert_eq!((user.language, user.theme), (Language::De, Theme::Dark));
        assert!(
            set_preferences(&pool, user.id, Language::En, Theme::Light, 7)
                .await
                .unwrap()
        );
        let user = get_by_id(&pool, user.id).await.unwrap().unwrap();
        assert_eq!((user.language, user.theme), (Language::En, Theme::Light));
        assert_eq!(user.default_expiry_days, 7);
        assert!(matches!(
            set_preferences(&pool, user.id, Language::En, Theme::Light, -1).await,
            Err(RepoError::Db(_))
        ));
    }

    #[tokio::test]
    async fn quota_can_be_set_and_resolves_against_the_default() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        assert_eq!(user.effective_quota(1000), Some(1000));
        assert_eq!(user.effective_quota(0), None);

        assert!(set_quota(&pool, user.id, Some(0)).await.unwrap());
        let user = get_by_id(&pool, user.id).await.unwrap().unwrap();
        assert_eq!(user.effective_quota(1000), None);

        assert!(set_quota(&pool, user.id, Some(42)).await.unwrap());
        let user = get_by_id(&pool, user.id).await.unwrap().unwrap();
        assert_eq!(user.effective_quota(0), Some(42));

        assert!(set_quota(&pool, user.id, None).await.unwrap());
        assert!(!set_quota(&pool, 999, None).await.unwrap());
        assert!(matches!(
            set_quota(&pool, user.id, Some(-1)).await,
            Err(RepoError::Db(_))
        ));
    }

    #[tokio::test]
    async fn create_and_read_with_defaults() {
        let (_dir, pool) = test_util::pool().await;
        let created = create(
            &pool,
            &NewUser {
                username: "Alice",
                password_hash: Some("$argon2id$x"),
                role: Role::Admin,
            },
            42,
        )
        .await
        .unwrap();

        assert_eq!(created.username, "Alice");
        assert_eq!(created.role, Role::Admin);
        assert_eq!(created.language, Language::De);
        assert_eq!(created.theme, Theme::Dark);
        assert_eq!(created.default_expiry_days, 0);
        assert_eq!(created.quota_bytes, None);
        assert_eq!(created.created_at, 42);
        assert!(!created.is_disabled());

        assert_eq!(
            get_by_id(&pool, created.id).await.unwrap(),
            Some(created.clone())
        );
        assert_eq!(
            get_by_username(&pool, "alice").await.unwrap(),
            Some(created)
        );
        assert_eq!(get_by_username(&pool, "bob").await.unwrap(), None);
        assert_eq!(count(&pool).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn first_admin_only_while_empty() {
        let (_dir, pool) = test_util::pool().await;
        let admin = create_first_admin(&pool, "root", "$argon2id$x", 5)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(admin.role, Role::Admin);
        assert_eq!(
            create_first_admin(&pool, "other", "$argon2id$x", 6)
                .await
                .unwrap(),
            None
        );
        assert_eq!(count(&pool).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn usernames_are_unique_case_insensitively() {
        let (_dir, pool) = test_util::pool().await;
        test_util::user(&pool, "alice").await;
        let err = create(
            &pool,
            &NewUser {
                username: "ALICE",
                password_hash: None,
                role: Role::User,
            },
            1,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RepoError::Conflict));
    }

    #[tokio::test]
    async fn rejects_empty_username() {
        let (_dir, pool) = test_util::pool().await;
        let err = create(
            &pool,
            &NewUser {
                username: "",
                password_hash: None,
                role: Role::User,
            },
            1,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RepoError::Db(_)));
    }

    #[tokio::test]
    async fn list_is_sorted_by_name() {
        let (_dir, pool) = test_util::pool().await;
        test_util::user(&pool, "carol").await;
        test_util::user(&pool, "alice").await;
        let names: Vec<_> = list(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.username)
            .collect();
        assert_eq!(names, ["alice", "carol"]);
    }

    #[tokio::test]
    async fn disable_and_enable() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        assert!(set_disabled(&pool, user.id, Some(7)).await.unwrap());
        assert_eq!(
            get_by_id(&pool, user.id)
                .await
                .unwrap()
                .unwrap()
                .disabled_at,
            Some(7)
        );
        assert!(set_disabled(&pool, user.id, None).await.unwrap());
        assert!(
            !get_by_id(&pool, user.id)
                .await
                .unwrap()
                .unwrap()
                .is_disabled()
        );
        assert!(!set_disabled(&pool, 999, Some(1)).await.unwrap());
    }

    #[tokio::test]
    async fn delete_removes_user() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        assert!(delete(&pool, user.id).await.unwrap());
        assert!(!delete(&pool, user.id).await.unwrap());
        assert_eq!(get_by_id(&pool, user.id).await.unwrap(), None);
    }
}
