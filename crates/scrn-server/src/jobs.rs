//! Background maintenance. One pass runs at startup and then every
//! [`MAINTENANCE_INTERVAL`]:
//!
//! - public links whose expiry has passed become private (the short ID is
//!   deleted, the image stays in the gallery);
//! - expired sessions are deleted;
//! - image files without a database row are removed once they are older
//!   than [`ORPHAN_MIN_AGE`]. Such files are left behind when the server
//!   stops between writing an upload and storing its row. The age guard
//!   keeps uploads that are still in progress.
//!
//! The public route checks the expiry on every request as well, so a link
//! never works past its expiry, even between two passes.

use std::path::Path;
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::AppState;
use crate::repo::{RepoError, screenshots, sessions};

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
/// screenshot and were last modified at least `min_age` before `now`.
/// Other files are never touched.
pub async fn remove_orphans(
    state: &AppState,
    dir: &Path,
    now: SystemTime,
    min_age: Duration,
) -> Result<u64, RepoError> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(error = %err, "cannot list the files directory");
            return Ok(0);
        }
    };
    let mut removed = 0;
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(error = %err, "listing the files directory failed");
                break;
            }
        };
        let Some(id) = entry.file_name().to_str().and_then(screenshot_id) else {
            continue;
        };
        let old_enough = entry
            .metadata()
            .await
            .ok()
            .filter(|meta| meta.is_file())
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= min_age);
        if !old_enough || screenshots::exists(&state.db, id).await? {
            continue;
        }
        match tokio::fs::remove_file(entry.path()).await {
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
