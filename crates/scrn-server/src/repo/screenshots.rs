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
    /// How often the current short link was opened; 0 for private ones.
    pub views: i64,
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
    /// The API token that uploaded it.
    pub token_id: Option<i64>,
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
    views: i64,
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
            views: row.views,
        })
    }
}

/// Selected columns, shared by all queries of this module.
macro_rules! columns {
    () => {
        "id, user_id, visibility, short_id, width, height, bytes, \
    thumb_bytes, sha256, created_at, expires_at, views"
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
         (id, user_id, token_id, visibility, short_id, width, height, bytes, thumb_bytes, sha256, created_at, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING ",
        columns!()
    );
    let row: Row = sqlx::query_as(sql)
        .bind(new.id.to_string())
        .bind(new.user_id)
        .bind(new.token_id)
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

/// Restricts a listing to the screenshots of one device group: those
/// uploaded with a token that is in the group now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupFilter {
    pub group_id: i64,
    /// Screenshots without a token count as part of the default group.
    pub is_default: bool,
}

/// Screenshots of a user, newest first, optionally filtered by visibility
/// and device group.
pub async fn list_for_user(
    db: impl SqliteExecutor<'_>,
    user_id: i64,
    visibility: Option<Visibility>,
    group: Option<GroupFilter>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Screenshot>, RepoError> {
    let sql = concat!(
        "SELECT ",
        columns!(),
        " FROM screenshots \
         WHERE user_id = ? AND (? IS NULL OR visibility = ?) \
         AND (? IS NULL \
              OR token_id IN (SELECT id FROM api_tokens WHERE group_id = ?) \
              OR (? AND token_id IS NULL)) \
         ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
    );
    let group_id = group.map(|g| g.group_id);
    let rows: Vec<Row> = sqlx::query_as(sql)
        .bind(user_id)
        .bind(visibility)
        .bind(visibility)
        .bind(group_id)
        .bind(group_id)
        .bind(group.is_some_and(|g| g.is_default))
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
        "UPDATE screenshots SET visibility = 'public', short_id = ?, expires_at = ?, views = 0 \
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

/// Changes the expiry of a public screenshot of `user_id`. Returns false if
/// it does not exist, belongs to someone else or is private.
pub async fn set_expiry(
    db: impl SqliteExecutor<'_>,
    id: Uuid,
    user_id: i64,
    expires_at: Option<i64>,
) -> Result<bool, RepoError> {
    let result = sqlx::query(
        "UPDATE screenshots SET expires_at = ? \
         WHERE id = ? AND user_id = ? AND visibility = 'public'",
    )
    .bind(expires_at)
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
        "UPDATE screenshots \
         SET visibility = 'private', short_id = NULL, expires_at = NULL, views = 0 \
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

/// Turns every public screenshot whose link has expired at `now` into a
/// private one: the short ID and the expiry are deleted, the image stays.
/// Returns the number of screenshots changed.
pub async fn expire_due(db: impl SqliteExecutor<'_>, now: i64) -> Result<u64, RepoError> {
    let result = sqlx::query(
        "UPDATE screenshots \
         SET visibility = 'private', short_id = NULL, expires_at = NULL, views = 0 \
         WHERE visibility = 'public' AND expires_at IS NOT NULL AND expires_at <= ?",
    )
    .bind(now)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// Counts one opening of the public link `short_id`. Does nothing if the
/// screenshot is private or gone by now.
pub async fn count_view(db: impl SqliteExecutor<'_>, short_id: &str) -> Result<(), RepoError> {
    sqlx::query(
        "UPDATE screenshots SET views = views + 1 \
         WHERE short_id = ? AND visibility = 'public'",
    )
    .bind(short_id)
    .execute(db)
    .await?;
    Ok(())
}

/// The owner of a screenshot, or `None` if no screenshot has this ID.
pub async fn owner(db: impl SqliteExecutor<'_>, id: Uuid) -> Result<Option<i64>, RepoError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM screenshots WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(db)
        .await?;
    Ok(row.map(|(user_id,)| user_id))
}

/// Storage used by a user in bytes: images plus thumbnails.
pub async fn usage_for_user(db: impl SqliteExecutor<'_>, user_id: i64) -> Result<i64, RepoError> {
    let (used,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(bytes + thumb_bytes), 0) FROM screenshots WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(used)
}

/// Storage used per user, for the admin page. Users without screenshots
/// are missing from the map.
pub async fn usage_by_user(
    db: impl SqliteExecutor<'_>,
) -> Result<std::collections::HashMap<i64, i64>, RepoError> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT user_id, SUM(bytes + thumb_bytes) FROM screenshots GROUP BY user_id",
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
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
            token_id: None,
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
    async fn expiry_can_only_be_set_on_own_public_screenshots() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        let public = insert(&pool, &new(alice.id, Some(link("abcdefgh", None))), 100)
            .await
            .unwrap();
        let private = insert(&pool, &new(alice.id, None), 100).await.unwrap();

        assert!(
            set_expiry(&pool, public.id, alice.id, Some(500))
                .await
                .unwrap()
        );
        let got = get_for_user(&pool, public.id, alice.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.public.unwrap().expires_at, Some(500));
        assert!(set_expiry(&pool, public.id, alice.id, None).await.unwrap());

        assert!(!set_expiry(&pool, public.id, bob.id, Some(1)).await.unwrap());
        // A private screenshot cannot get an expiry (the CHECK forbids it).
        assert!(
            !set_expiry(&pool, private.id, alice.id, Some(1))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn expire_due_only_touches_expired_public_links() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let due = insert(&pool, &new(user.id, Some(link("dueeeeee", Some(200)))), 100)
            .await
            .unwrap();
        let later = insert(&pool, &new(user.id, Some(link("laterrrr", Some(300)))), 100)
            .await
            .unwrap();
        let never = insert(&pool, &new(user.id, Some(link("neverrrr", None))), 100)
            .await
            .unwrap();
        let private = insert(&pool, &new(user.id, None), 100).await.unwrap();

        assert_eq!(expire_due(&pool, 199).await.unwrap(), 0);
        assert_eq!(expire_due(&pool, 200).await.unwrap(), 1);
        let get = |id| get_for_user(&pool, id, user.id);
        assert_eq!(get(due.id).await.unwrap().unwrap().public, None);
        assert!(get(later.id).await.unwrap().unwrap().public.is_some());
        assert!(get(never.id).await.unwrap().unwrap().public.is_some());
        assert_eq!(get(private.id).await.unwrap().unwrap().public, None);
        assert_eq!(get_by_short_id(&pool, "dueeeeee").await.unwrap(), None);
        // The image itself is kept.
        assert_eq!(owner(&pool, due.id).await.unwrap(), Some(due.user_id));
        assert_eq!(owner(&pool, Uuid::new_v4()).await.unwrap(), None);

        assert_eq!(expire_due(&pool, 10_000).await.unwrap(), 1);
        assert!(get(never.id).await.unwrap().unwrap().public.is_some());
    }

    #[tokio::test]
    async fn usage_sums_images_and_thumbnails_per_user() {
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let bob = test_util::user(&pool, "bob").await;
        assert_eq!(usage_for_user(&pool, alice.id).await.unwrap(), 0);
        let shot = insert(&pool, &new(alice.id, None), 100).await.unwrap();
        insert(&pool, &new(alice.id, None), 100).await.unwrap();
        insert(&pool, &new(bob.id, None), 100).await.unwrap();
        assert_eq!(usage_for_user(&pool, alice.id).await.unwrap(), 2200);
        let all = usage_by_user(&pool).await.unwrap();
        assert_eq!(all.get(&alice.id), Some(&2200));
        assert_eq!(all.get(&bob.id), Some(&1100));
        delete_for_user(&pool, shot.id, alice.id).await.unwrap();
        assert_eq!(usage_for_user(&pool, alice.id).await.unwrap(), 1100);
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
            ids(list_for_user(&pool, alice.id, None, None, 10, 0)
                .await
                .unwrap()),
            [c.id, b.id, a.id]
        );
        assert_eq!(
            ids(
                list_for_user(&pool, alice.id, Some(Visibility::Public), None, 10, 0)
                    .await
                    .unwrap()
            ),
            [b.id]
        );
        assert_eq!(
            ids(
                list_for_user(&pool, alice.id, Some(Visibility::Private), None, 10, 0)
                    .await
                    .unwrap()
            ),
            [c.id, a.id]
        );
        assert_eq!(
            ids(list_for_user(&pool, alice.id, None, None, 1, 0)
                .await
                .unwrap()),
            [c.id]
        );
        assert_eq!(
            ids(list_for_user(&pool, alice.id, None, None, 2, 1)
                .await
                .unwrap()),
            [b.id, a.id]
        );
    }

    #[tokio::test]
    async fn list_filters_by_device_group() {
        use crate::repo::{api_tokens, device_groups};
        let (_dir, pool) = test_util::pool().await;
        let alice = test_util::user(&pool, "alice").await;
        let default = device_groups::list_for_user(&pool, alice.id).await.unwrap()[0].id;
        let work = device_groups::create(&pool, alice.id, "Work", 1)
            .await
            .unwrap();
        let laptop = api_tokens::create(
            &pool,
            alice.id,
            Some(work),
            "laptop",
            &test_util::hash(1),
            1,
        )
        .await
        .unwrap();
        let desktop = api_tokens::create(&pool, alice.id, None, "desktop", &test_util::hash(2), 1)
            .await
            .unwrap();
        let upload = |token: Option<i64>| NewScreenshot {
            token_id: token,
            ..new(alice.id, None)
        };
        let old = insert(&pool, &upload(None), 100).await.unwrap();
        let from_desktop = insert(&pool, &upload(Some(desktop.id)), 200).await.unwrap();
        let from_laptop = insert(&pool, &upload(Some(laptop.id)), 300).await.unwrap();

        let in_group = |group_id, is_default| {
            let pool = pool.clone();
            async move {
                let filter = GroupFilter {
                    group_id,
                    is_default,
                };
                list_for_user(&pool, alice.id, None, Some(filter), 10, 0)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|s| s.id)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(in_group(work, false).await, [from_laptop.id]);
        assert_eq!(in_group(default, true).await, [from_desktop.id, old.id]);

        // Moving the token moves its screenshots.
        assert!(
            api_tokens::set_group(&pool, laptop.id, alice.id, default)
                .await
                .unwrap()
        );
        assert!(in_group(work, false).await.is_empty());
        assert_eq!(in_group(default, true).await.len(), 3);
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

    async fn views(pool: &sqlx::SqlitePool, id: Uuid) -> i64 {
        let (views,): (i64,) = sqlx::query_as("SELECT views FROM screenshots WHERE id = ?")
            .bind(id.to_string())
            .fetch_one(pool)
            .await
            .unwrap();
        views
    }

    #[tokio::test]
    async fn views_count_the_current_link_only() {
        let (_dir, pool) = test_util::pool().await;
        let user = test_util::user(&pool, "alice").await;
        let shot = insert(&pool, &new(user.id, Some(link("Views123", Some(500)))), 100)
            .await
            .unwrap();
        let private = insert(&pool, &new(user.id, None), 100).await.unwrap();
        assert_eq!(shot.views, 0);

        count_view(&pool, "Views123").await.unwrap();
        count_view(&pool, "Views123").await.unwrap();
        count_view(&pool, "Unknown1").await.unwrap();
        assert_eq!(views(&pool, shot.id).await, 2);
        assert_eq!(views(&pool, private.id).await, 0);
        // The expiry is part of the same link and keeps the count.
        assert!(set_expiry(&pool, shot.id, user.id, None).await.unwrap());
        assert_eq!(views(&pool, shot.id).await, 2);

        // A new link starts from zero, and the old one counts nothing.
        assert!(
            set_public(&pool, shot.id, user.id, &link("Views456", None))
                .await
                .unwrap()
        );
        assert_eq!(views(&pool, shot.id).await, 0);
        count_view(&pool, "Views123").await.unwrap();
        count_view(&pool, "Views456").await.unwrap();
        assert_eq!(views(&pool, shot.id).await, 1);

        assert!(set_private(&pool, shot.id, user.id).await.unwrap());
        assert_eq!(views(&pool, shot.id).await, 0);
        count_view(&pool, "Views456").await.unwrap();
        assert_eq!(views(&pool, shot.id).await, 0);

        // Expired links lose their count as well.
        assert!(
            set_public(&pool, shot.id, user.id, &link("Views789", Some(200)))
                .await
                .unwrap()
        );
        count_view(&pool, "Views789").await.unwrap();
        assert_eq!(expire_due(&pool, 300).await.unwrap(), 1);
        assert_eq!(views(&pool, shot.id).await, 0);
    }
}
