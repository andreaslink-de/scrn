//! Operations on screenshots shared by the API and the gallery.

use std::path::Path;

use uuid::Uuid;

use crate::crypto::CryptoError;
use crate::repo::screenshots::{self, PublicLink};
use crate::repo::{RepoError, Sha256};
use crate::{short_id, storage};

/// Attempts to find an unused short ID. With 62^8 possible IDs a single
/// collision is already very unlikely.
pub const SHORT_ID_ATTEMPTS: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum ShotError {
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error("no unused short ID found")]
    NoShortId,
}

/// Makes a screenshot of `user_id` public under a newly generated short ID,
/// replacing any previous one. Returns the new link, or `None` if the
/// screenshot does not exist or belongs to someone else.
pub async fn make_public(
    db: &sqlx::SqlitePool,
    id: Uuid,
    user_id: i64,
    expires_at: Option<i64>,
) -> Result<Option<PublicLink>, ShotError> {
    for _ in 0..SHORT_ID_ATTEMPTS {
        let link = PublicLink {
            short_id: short_id::generate()?,
            expires_at,
        };
        match screenshots::set_public(db, id, user_id, &link).await {
            Ok(true) => return Ok(Some(link)),
            Ok(false) => return Ok(None),
            Err(RepoError::Conflict) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Err(ShotError::NoShortId)
}

/// Deletes a screenshot of `user_id` with its files. Returns false if it
/// does not exist or belongs to someone else. Once the row is gone the
/// image is unreachable, so a file that cannot be removed is only logged.
pub async fn delete(
    db: &sqlx::SqlitePool,
    files_dir: &Path,
    id: Uuid,
    user_id: i64,
) -> Result<bool, RepoError> {
    if !screenshots::delete_for_user(db, id, user_id).await? {
        return Ok(false);
    }
    if let Err(err) = storage::remove(files_dir, user_id, id).await {
        tracing::warn!(screenshot_id = %id, error = %err, "removing screenshot files failed");
    }
    tracing::info!(screenshot_id = %id, user_id, "screenshot deleted");
    Ok(true)
}

/// ETag of a stored image.
pub fn etag(sha256: &Sha256) -> String {
    format!("\"{}\"", crate::crypto::hex_encode(sha256))
}
