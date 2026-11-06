//! Background maintenance. One pass runs at startup and then every
//! [`MAINTENANCE_INTERVAL`]:
//!
//! - public links whose expiry has passed become private (the short ID is
//!   deleted, the image stays in the gallery);
//! - expired sessions are deleted;
//! - image files without a matching database row are removed once they are
//!   older than [`ORPHAN_MIN_AGE`]. Such files are left behind when the server
//!   stops between writing an upload and storing its row. The age guard
//!   keeps uploads that are still in progress.
//!
//! The public route checks the expiry on every request as well, so a link
//! never works past its expiry, even between two passes.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::repo::{RepoError, screenshots, sessions};
use crate::{AppState, storage};

pub const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub const ORPHAN_MIN_AGE: Duration = Duration::from_secs(60 * 60);

/// What one pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub expired_links: u64,
    pub expired_sessions: u64,
    pub orphan_files: u64,
}

/// Runs one maintenance pass. File system time is used for the orphan age,
/// the injected clock for everything stored in the database.
pub async fn run_once(state: &AppState) -> Result<Report, RepoError> {
    let now = state.clock.now();
    let expired_links = screenshots::expire_due(&state.db, now).await?;
    let expired_sessions = sessions::delete_expired(&state.db, now).await?;
    let orphan_files = remove_orphans(
        state,
        &state.config.files_dir(),
        SystemTime::now(),
        ORPHAN_MIN_AGE,
    )
    .await?;
    Ok(Report {
        expired_links,
        expired_sessions,
        orphan_files,
    })
}

/// Starts the periodic job. It runs until the runtime shuts down.
pub fn spawn(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(MAINTENANCE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            // The first tick completes immediately.
            ticker.tick().await;
            match run_once(&state).await {
                Ok(report) if report != Report::default() => tracing::info!(
                    expired_links = report.expired_links,
                    expired_sessions = report.expired_sessions,
                    orphan_files = report.orphan_files,
                    "maintenance pass"
                ),
                Ok(_) => {}
                Err(err) => tracing::error!(error = %err, "maintenance pass failed"),
            }
        }
    })
}

/// Removes `<uuid>.png` and `<uuid>.thumb.png` files that belong to no
/// screenshot of the user whose directory they are in and were last
/// modified at least `min_age` before `now`. Files directly in `dir` are
/// left over from the earlier flat layout and are removed only if no
/// screenshot has their ID. Other files are never touched.
pub async fn remove_orphans(
    state: &AppState,
    dir: &Path,
    now: SystemTime,
    min_age: Duration,
) -> Result<u64, RepoError> {
    let mut removed = sweep(state, dir, None, now, min_age).await?;
    for (user_id, path) in user_dirs(dir).await {
        removed += sweep(state, &path, Some(user_id), now, min_age).await?;
    }
    Ok(removed)
}

/// Removes the orphans in one directory. `owner` is the user the directory
/// belongs to, `None` for the top level.
async fn sweep(
    state: &AppState,
    dir: &Path,
    owner: Option<i64>,
    now: SystemTime,
    min_age: Duration,
) -> Result<u64, RepoError> {
    let mut removed = 0;
    for (id, path, meta) in screenshot_files(dir).await {
        let old_enough = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= min_age);
        if !old_enough {
            continue;
        }
        let actual = screenshots::owner(&state.db, id).await?;
        let keep = match owner {
            Some(user_id) => actual == Some(user_id),
            None => actual.is_some(),
        };
        if keep {
            continue;
        }
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                removed += 1;
                tracing::info!(screenshot_id = %id, "removed orphaned file");
            }
            Err(err) => {
                tracing::warn!(screenshot_id = %id, error = %err, "removing orphaned file failed")
            }
        }
    }
    Ok(removed)
}

/// Moves files of the earlier flat layout (`files/<id>.png`) into the
/// directory of their owner. Runs at startup before requests are served;
/// files without a screenshot stay for [`remove_orphans`]. A file that
/// cannot be moved is logged and tried again on the next start.
pub async fn move_legacy_files(db: &sqlx::SqlitePool, dir: &Path) -> Result<u64, RepoError> {
    let mut moved = 0;
    for (id, path, _) in screenshot_files(dir).await {
        let Some(user_id) = screenshots::owner(db, id).await? else {
            continue;
        };
        let Some(name) = path.file_name() else {
            continue;
        };
        let target_dir = storage::user_dir(dir, user_id);
        let result = match tokio::fs::create_dir_all(&target_dir).await {
            Ok(()) => tokio::fs::rename(&path, target_dir.join(name)).await,
            Err(err) => Err(err),
        };
        match result {
            Ok(()) => moved += 1,
            Err(err) => {
                tracing::warn!(screenshot_id = %id, error = %err, "moving a file into its user directory failed")
            }
        }
    }
    if moved > 0 {
        tracing::info!(files = moved, "moved files into user directories");
    }
    Ok(moved)
}

/// The regular files in `dir` whose names we write, with their IDs.
async fn screenshot_files(dir: &Path) -> Vec<(Uuid, PathBuf, Metadata)> {
    let mut files = Vec::new();
    for (name, path, meta) in list(dir).await {
        if let Some(id) = screenshot_id(&name).filter(|_| meta.is_file()) {
            files.push((id, path, meta));
        }
    }
    files
}

/// The user directories in `dir`: subdirectories named by a user ID.
async fn user_dirs(dir: &Path) -> Vec<(i64, PathBuf)> {
    let mut dirs = Vec::new();
    for (name, path, meta) in list(dir).await {
        let user_id = name
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0 && id.to_string() == name);
        if let Some(user_id) = user_id.filter(|_| meta.is_dir()) {
            dirs.push((user_id, path));
        }
    }
    dirs
}

/// Entries of `dir` with UTF-8 names. Listing errors are logged, and what
/// was read until then is returned.
async fn list(dir: &Path) -> Vec<(String, PathBuf, Metadata)> {
    let mut found = Vec::new();
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(error = %err, "cannot list the files directory");
            return found;
        }
    };
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(error = %err, "listing the files directory failed");
                break;
            }
        };
        // `DirEntry::metadata` does not follow symbolic links, so a link
        // is neither a file nor a directory here and is never touched.
        let (Ok(name), Ok(meta)) = (entry.file_name().into_string(), entry.metadata().await) else {
            continue;
        };
        found.push((name, entry.path(), meta));
    }
    found
}

/// The screenshot ID of a file name we write, if it is one.
fn screenshot_id(name: &str) -> Option<Uuid> {
    let stem = name
        .strip_suffix(".thumb.png")
        .or_else(|| name.strip_suffix(".png"))?;
    // Only the canonical form we write: hyphenated lowercase.
    let id = Uuid::parse_str(stem).ok()?;
    (id.to_string() == stem).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        let id = Uuid::new_v4();
        assert_eq!(screenshot_id(&format!("{id}.png")), Some(id));
        assert_eq!(screenshot_id(&format!("{id}.thumb.png")), Some(id));
        let upper = id.to_string().to_uppercase();
        for name in [
            format!("{upper}.png"),
            format!("{id}.jpg"),
            format!("{id}"),
            format!("{}.png", id.simple()),
            "notes.txt".to_owned(),
            ".png".to_owned(),
        ] {
            assert_eq!(screenshot_id(&name), None, "{name}");
        }
    }
}
