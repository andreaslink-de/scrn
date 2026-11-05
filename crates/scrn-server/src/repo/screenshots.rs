//! The `screenshots` table. The image files live under `/data/files`.
//!
//! A screenshot is either private (no short ID, no expiry) or public with a
//! short ID and an optional expiry. The database enforces this with CHECK
//! constraints, and [`PublicLink`] makes the invalid combinations
//! unrepresentable in Rust.

use sqlx::{FromRow, SqliteExecutor};
use uuid::Uuid;

use super::{RepoError, Sha256, to_sha256};

/// Who can see a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum Visibility {
    Private,
    Public,
}

/// The short link of a public screenshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLink {
    pub short_id: String,
    /// `None` means the link never expires.
    pub expires_at: Option<i64>,
}

/// A screenshot record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screenshot {
    pub id: Uuid,
    pub user_id: i64,
    /// `Some` for public screenshots, `None` for private ones.
    pub public: Option<PublicLink>,
    pub width: i64,
    pub height: i64,
    pub bytes: i64,
    pub thumb_bytes: i64,
    pub sha256: Sha256,
    pub created_at: i64,
}

impl Screenshot {
    pub fn visibility(&self) -> Visibility {
        if self.public.is_some() {
            Visibility::Public
        } else {
            Visibility::Private
        }
    }
}

/// Values for a new screenshot.
#[derive(Debug, Clone)]
pub struct NewScreenshot {
    pub id: Uuid,
    pub user_id: i64,
    pub public: Option<PublicLink>,
    pub width: i64,
    pub height: i64,
    pub bytes: i64,
    pub thumb_bytes: i64,
    pub sha256: Sha256,
}

#[derive(FromRow)]
struct Row {
    id: String,
    user_id: i64,
    visibility: Visibility,
    short_id: Option<String>,
    width: i64,
    height: i64,
    bytes: i64,
    thumb_bytes: i64,
    sha256: Vec<u8>,
    created_at: i64,
    expires_at: Option<i64>,
}

impl TryFrom<Row> for Screenshot {
    type Error = RepoError;

    fn try_from(row: Row) -> Result<Self, Self::Error> {
        let id = Uuid::parse_str(&row.id).map_err(|_| RepoError::InvalidData { column: "id" })?;
        let public = match (row.visibility, row.short_id) {
            (Visibility::Public, Some(short_id)) => Some(PublicLink {
                short_id,
                expires_at: row.expires_at,
            }),
            (Visibility::Private, None) => None,
            _ => return Err(RepoError::InvalidData { column: "short_id" }),
        };
        Ok(Self {
            id,
            user_id: row.user_id,
            public,
            width: row.width,
            height: row.height,
            bytes: row.bytes,
            thumb_bytes: row.thumb_bytes,
            sha256: to_sha256("sha256", row.sha256)?,
            created_at: row.created_at,
        })
    }
}

/// Selected columns, shared by all queries of this module.
macro_rules! columns {
    () => {
        "id, user_id, visibility, short_id, width, height, bytes, \
    thumb_bytes, sha256, created_at, expires_at"
    };
}

fn convert(row: Option<Row>) -> Result<Option<Screenshot>, RepoError> {
    row.map(Screenshot::try_from).transpose()
}

/// Inserts a screenshot. Returns [`RepoError::Conflict`] if the ID or the
/// short ID already exists, so the caller can retry with a new short ID.
pub async fn insert(
    db: impl SqliteExecutor<'_>,
    new: &NewScreenshot,
    now: i64,
) -> Result<Screenshot, RepoError> {
    let (visibility, short_id, expires_at) = split_link(new.public.as_ref());
    let sql = concat!(
        "INSERT INTO screenshots \
         (id, user_id, visibility, short_id, width, height, bytes, thumb_bytes, sha256, created_at, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING ",
        columns!()
    );
    let row: Row = sqlx::query_as(sql)
        .bind(new.id.to_string())
        .bind(new.user_id)
        .bind(visibility)
        .bind(short_id)
        .bind(new.width)
        .bind(new.height)
        .bind(new.bytes)
        .bind(new.thumb_bytes)
        .bind(&new.sha256[..])
        .bind(now)
        .bind(expires_at)
        .fetch_one(db)
        .await?;
    row.try_into()
}

/// Returns the screenshot only if it belongs to `user_id`.
pub async fn get_for_user(
    db: impl SqliteExecutor<'_>,
    id: Uuid,
    user_id: i64,
) -> Result<Option<Screenshot>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM screenshots WHERE id = ? AND user_id = ?"
    );
    convert(
        sqlx::query_as(sql)
            .bind(id.to_string())
            .bind(user_id)
            .fetch_optional(db)
            .await?,
    )
}

/// Looks up a screenshot by its short ID. Only public screenshots have one,
/// so a private screenshot can never be found this way. Expiry is checked
/// by the caller.
pub async fn get_by_short_id(
    db: impl SqliteExecutor<'_>,
    short_id: &str,
) -> Result<Option<Screenshot>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM screenshots WHERE short_id = ? AND visibility = 'public'"
    );
    convert(
        sqlx::query_as(sql)
            .bind(short_id)
            .fetch_optional(db)
            .await?,
    )
}

/// Screenshots of a user, newest first, optionally filtered by visibility.
pub async fn list_for_user(
    db: impl SqliteExecutor<'_>,
    user_id: i64,
    visibility: Option<Visibility>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Screenshot>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM screenshots \
         WHERE user_id = ? AND (? IS NULL OR visibility = ?) \
         ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
    );
    let rows: Vec<Row> = sqlx::query_as(sql)
        .bind(user_id)
        .bind(visibility)
        .bind(visibility)
        .bind(limit)
        .bind(offset)
        .fetch_all(db)
        .await?;
    rows.into_iter().map(Screenshot::try_from).collect()
}

/// Makes a screenshot of `user_id` public with the given link, replacing any
/// previous short ID. Returns false if it does not exist or belongs to
/// someone else, and [`RepoError::Conflict`] if the short ID is taken.
pub async fn set_public(
    db: impl SqliteExecutor<'_>,
    id: Uuid,
    user_id: i64,
    link: &PublicLink,
) -> Result<bool, RepoError> {
    let result = sqlx::query(
        "UPDATE screenshots SET visibility = 'public', short_id = ?, expires_at = ? \
         WHERE id = ? AND user_id = ?",
    )
    .bind(&link.short_id)
    .bind(link.expires_at)
    .bind(id.to_string())
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Makes a screenshot of `user_id` private. The short ID and the expiry are
/// deleted, so the old link stops working. Returns false if it does not
/// exist or belongs to someone else.
pub async fn set_private(
    db: impl SqliteExecutor<'_>,
    id: Uuid,
    user_id: i64,
) -> Result<bool, RepoError> {
    let result = sqlx::query(
        "UPDATE screenshots SET visibility = 'private', short_id = NULL, expires_at = NULL \
         WHERE id = ? AND user_id = ?",
    )
    .bind(id.to_string())
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Deletes the record of a screenshot of `user_id`. The caller removes the
/// files. Returns false if it does not exist or belongs to someone else.
pub async fn delete_for_user(
    db: impl SqliteExecutor<'_>,
    id: Uuid,
    user_id: i64,
) -> Result<bool, RepoError> {
    let result = sqlx::query("DELETE FROM screenshots WHERE id = ? AND user_id = ?")
        .bind(id.to_string())
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

fn split_link(link: Option<&PublicLink>) -> (Visibility, Option<&str>, Option<i64>) {
    match link {
        Some(link) => (
            Visibility::Public,
            Some(link.short_id.as_str()),
            link.expires_at,
        ),
        None => (Visibility::Private, None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{test_util, users};

    fn new(user_id: i64, public: Option<PublicLink>) -> NewScreenshot {
        NewScreenshot {
            id: Uuid::new_v4(),
            user_id,
            public,
            width: 800,
            height: 600,
            bytes: 1000,
            thumb_bytes: 100,
            sha256: test_util::hash(9),
        }
    }

    fn link(short_id: &str, expires_at: Option<i64>) -> PublicLink {
        PublicLink {
            short_id: short_id.to_owned(),
            expires_at,
        }
    }

    #[tokio::test]
    async fn insert_and_read_private() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let shot = insert(&pool, &new(user.id, None), 100).await.unwrap();
        assert_eq!(shot.visibility(), Visibility::Private);
        assert_eq!(shot.created_at, 100);
        assert_eq!(
            get_for_user(&pool, shot.id, user.id).await.unwrap(),
            Some(shot)
        );
    }

    #[tokio::test]
    async fn insert_and_read_public() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let shot = insert(&pool, &new(user.id, Some(link("Abc12345", Some(500)))), 100)
            .await
            .unwrap();
        assert_eq!(shot.visibility(), Visibility::Public);
        assert_eq!(shot.public, Some(link("Abc12345", Some(500))));
        assert_eq!(
            get_by_short_id(&pool, "Abc12345").await.unwrap(),
            Some(shot)
        );
        assert_eq!(get_by_short_id(&pool, "abc12345").await.unwrap(), None);
    }

    #[tokio::test]
    async fn other_users_cannot_read_change_or_delete() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        let shot = insert(&pool, &new(alice.id, None), 100).await.unwrap();

        assert_eq!(get_for_user(&pool, shot.id, bob.id).await.unwrap(), None);
        assert!(
            !set_public(&pool, shot.id, bob.id, &link("Abc12345", None))
                .await
                .unwrap()
        );
        assert!(!set_private(&pool, shot.id, bob.id).await.unwrap());
        assert!(!delete_for_user(&pool, shot.id, bob.id).await.unwrap());
        assert!(
            get_for_user(&pool, shot.id, alice.id)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn short_ids_are_unique() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        insert(&pool, &new(user.id, Some(link("Abc12345", None))), 100)
            .await
            .unwrap();
        let err = insert(&pool, &new(user.id, Some(link("Abc12345", None))), 100)
            .await
            .unwrap_err();
        assert!(matches!(err, RepoError::Conflict));

        // Several private screenshots without a short ID are fine.
        insert(&pool, &new(user.id, None), 100).await.unwrap();
        insert(&pool, &new(user.id, None), 100).await.unwrap();
    }

    #[tokio::test]
    async fn database_rejects_short_id_on_private_screenshot() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let shot = insert(&pool, &new(user.id, None), 100).await.unwrap();

        // Bypass the typed API to check the CHECK constraints themselves.
        for sql in [
            "UPDATE screenshots SET short_id = 'Abc12345' WHERE id = ?",
            "UPDATE screenshots SET expires_at = 500 WHERE id = ?",
            "UPDATE screenshots SET visibility = 'public' WHERE id = ?",
        ] {
            let result = sqlx::query(sql)
                .bind(shot.id.to_string())
                .execute(&pool)
                .await;
            assert!(result.is_err(), "{sql} should violate a constraint");
        }

        let result = sqlx::query(
            "INSERT INTO screenshots \
             (id, user_id, visibility, short_id, width, height, bytes, thumb_bytes, sha256, created_at) \
             VALUES (?, ?, 'private', 'Abc12345', 1, 1, 1, 1, zeroblob(32), 1)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(user.id)
        .execute(&pool)
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn database_rejects_public_screenshot_without_short_id() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let shot = insert(&pool, &new(user.id, Some(link("Abc12345", None))), 100)
            .await
            .unwrap();
        let result = sqlx::query("UPDATE screenshots SET short_id = NULL WHERE id = ?")
            .bind(shot.id.to_string())
            .execute(&pool)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn private_kills_link_and_public_creates_new_one() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let shot = insert(&pool, &new(user.id, Some(link("Old12345", Some(500)))), 100)
            .await
            .unwrap();

        assert!(set_private(&pool, shot.id, user.id).await.unwrap());
        assert_eq!(get_by_short_id(&pool, "Old12345").await.unwrap(), None);
        let private = get_for_user(&pool, shot.id, user.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(private.public, None);

        assert!(
            set_public(&pool, shot.id, user.id, &link("New12345", None))
                .await
                .unwrap()
        );
        assert_eq!(get_by_short_id(&pool, "Old12345").await.unwrap(), None);
        let public = get_by_short_id(&pool, "New12345").await.unwrap().unwrap();
        assert_eq!(public.id, shot.id);
        assert_eq!(public.public, Some(link("New12345", None)));
    }

    #[tokio::test]
    async fn set_public_with_taken_short_id_conflicts() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        insert(&pool, &new(user.id, Some(link("Abc12345", None))), 100)
            .await
            .unwrap();
        let other = insert(&pool, &new(user.id, None), 100).await.unwrap();
        let err = set_public(&pool, other.id, user.id, &link("Abc12345", None))
            .await
            .unwrap_err();
        assert!(matches!(err, RepoError::Conflict));
    }

    #[tokio::test]
    async fn list_filters_and_pages_newest_first() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        let a = insert(&pool, &new(alice.id, None), 100).await.unwrap();
        let b = insert(&pool, &new(alice.id, Some(link("Abc12345", None))), 200)
            .await
            .unwrap();
        let c = insert(&pool, &new(alice.id, None), 300).await.unwrap();
        insert(&pool, &new(bob.id, None), 400).await.unwrap();

        let ids = |v: Vec<Screenshot>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(
            ids(list_for_user(&pool, alice.id, None, 10, 0).await.unwrap()),
            [c.id, b.id, a.id]
        );
        assert_eq!(
            ids(
                list_for_user(&pool, alice.id, Some(Visibility::Public), 10, 0)
                    .await
                    .unwrap()
            ),
            [b.id]
        );
        assert_eq!(
            ids(
                list_for_user(&pool, alice.id, Some(Visibility::Private), 10, 0)
                    .await
                    .unwrap()
            ),
            [c.id, a.id]
        );
        assert_eq!(
            ids(list_for_user(&pool, alice.id, None, 1, 0).await.unwrap()),
            [c.id]
        );
        assert_eq!(
            ids(list_for_user(&pool, alice.id, None, 2, 1).await.unwrap()),
            [b.id, a.id]
        );
    }

    #[tokio::test]
    async fn delete_and_cascade() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let first = insert(&pool, &new(user.id, None), 100).await.unwrap();
        let second = insert(&pool, &new(user.id, Some(link("Abc12345", None))), 100)
            .await
            .unwrap();

        assert!(delete_for_user(&pool, first.id, user.id).await.unwrap());
        assert!(!delete_for_user(&pool, first.id, user.id).await.unwrap());

        users::delete(&pool, user.id).await.unwrap();
        assert_eq!(get_by_short_id(&pool, "Abc12345").await.unwrap(), None);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM screenshots WHERE id = ?")
            .bind(second.id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    }
}
