//! Backup download for administrators: an uncompressed tar archive with a
//! consistent snapshot of the database (`scrn.db`) and all screenshot
//! files (`files/...`). It has the same layout as the archives of
//! `scripts/backup.sh`, so the same restore steps apply.
//!
//! The snapshot is taken with `VACUUM INTO` while the server keeps running.
//! The file list is read after the snapshot, so every screenshot in the
//! snapshot has its files (unless it is deleted in between). The archive is
//! streamed to the client and never stored on the server; only the
//! snapshot is written next to the database and removed afterwards.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use tokio::io::AsyncReadExt;
use tokio::sync::{OwnedSemaphorePermit, mpsc};

use crate::db::{self, BackupError};
use crate::tar::{self, TarError};
use crate::{crypto, storage};

/// Snapshots are written to the data directory under this prefix.
pub const SNAPSHOT_PREFIX: &str = ".backup-";
/// Bytes read from a file at a time.
const CHUNK: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error(transparent)]
    Snapshot(#[from] BackupError),
    #[error(transparent)]
    Tar(#[from] TarError),
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
    #[error("reading {0} failed: {1}")]
    Io(String, #[source] io::Error),
}

/// A file in the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Name in the archive, e.g. `files/1/<uuid>.png`.
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
}

/// A prepared archive. The snapshot file is removed when it is dropped.
pub struct Archive {
    pub entries: Vec<Entry>,
    /// Kept only so that the snapshot is removed with the archive.
    _snapshot: Snapshot,
}

impl Archive {
    /// The exact size of the archive in bytes.
    pub fn size(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| tar::entry_len(e.size))
            .sum::<u64>()
            + tar::END.len() as u64
    }
}

/// Removes the snapshot file when dropped.
struct Snapshot(PathBuf);

impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.0)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(error = %err, "removing the backup snapshot failed");
        }
    }
}

/// Takes the database snapshot and lists the files.
pub async fn prepare(
    pool: &sqlx::SqlitePool,
    data_dir: &Path,
    files_dir: &Path,
) -> Result<Archive, ArchiveError> {
    let name = format!(
        "{SNAPSHOT_PREFIX}{}.db",
        crypto::hex_encode(&crypto::random_bytes::<16>()?)
    );
    let snapshot = Snapshot(data_dir.join(name));
    db::backup(pool, &snapshot.0).await?;

    let mut entries = vec![entry("scrn.db".to_owned(), snapshot.0.clone()).await?];
    // Files left at the top level by the flat layout are kept as they are;
    // the server moves them into user directories after a restore.
    for (_, path, _) in storage::screenshot_files(files_dir).await {
        entries.push(entry(archive_name(files_dir, &path), path).await?);
    }
    let mut dirs = storage::user_dirs(files_dir).await;
    dirs.sort();
    for (_, dir) in dirs {
        let mut files = storage::screenshot_files(&dir).await;
        files.sort_by(|a, b| a.1.cmp(&b.1));
        for (_, path, _) in files {
            entries.push(entry(archive_name(files_dir, &path), path).await?);
        }
    }
    Ok(Archive {
        entries,
        _snapshot: snapshot,
    })
}

/// `files/<relative path>` with forward slashes.
fn archive_name(files_dir: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(files_dir).unwrap_or(path);
    let parts: Vec<&str> = relative
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    format!("files/{}", parts.join("/"))
}

async fn entry(name: String, path: PathBuf) -> Result<Entry, ArchiveError> {
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|err| ArchiveError::Io(name.clone(), err))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    // Fail now, before the response starts, if a header cannot be built.
    tar::header(&name, meta.len(), mtime)?;
    Ok(Entry {
        name,
        path,
        size: meta.len(),
        mtime,
    })
}

/// Streams the archive as a response body. `permit` is held until the
/// stream ends, so only one backup runs at a time.
pub fn body(archive: Archive, permit: OwnedSemaphorePermit) -> Body {
    let (tx, rx) = mpsc::channel::<io::Result<Bytes>>(4);
    tokio::spawn(async move {
        let started = SystemTime::now();
        match write(&archive, &tx).await {
            Ok(()) => tracing::info!(
                files = archive.entries.len() - 1,
                bytes = archive.size(),
                seconds = started.elapsed().map_or(0, |d| d.as_secs()),
                "backup download finished"
            ),
            Err(Abort::Client) => tracing::info!("backup download aborted by the client"),
            Err(Abort::Io(err)) => {
                tracing::error!(error = %err, "backup download failed");
                // Ends the response with an error, so the client sees an
                // incomplete download instead of a truncated archive.
                let _ = tx.send(Err(err)).await;
            }
        }
        // Clean up before the sender is dropped, which ends the body: once
        // the client has the whole archive, the snapshot is gone and the
        // next backup can start.
        drop(archive);
        drop(permit);
        drop(tx);
    });
    Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    }))
}

enum Abort {
    /// The client went away.
    Client,
    Io(io::Error),
}

async fn send(tx: &mpsc::Sender<io::Result<Bytes>>, data: Bytes) -> Result<(), Abort> {
    tx.send(Ok(data)).await.map_err(|_| Abort::Client)
}

async fn write(archive: &Archive, tx: &mpsc::Sender<io::Result<Bytes>>) -> Result<(), Abort> {
    let mut buffer = vec![0u8; CHUNK];
    for entry in &archive.entries {
        let header = tar::header(&entry.name, entry.size, entry.mtime)
            .map_err(|err| Abort::Io(io::Error::other(err)))?;
        let mut file = tokio::fs::File::open(&entry.path)
            .await
            .map_err(Abort::Io)?;
        send(tx, Bytes::copy_from_slice(&header)).await?;
        // Exactly the announced size: stored files never change, so a
        // different length means something is wrong.
        let mut left = entry.size;
        while left > 0 {
            let want = usize::try_from(left.min(CHUNK as u64)).unwrap_or(CHUNK);
            let read = file.read(&mut buffer[..want]).await.map_err(Abort::Io)?;
            if read == 0 {
                return Err(Abort::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("{} became shorter during the backup", entry.name),
                )));
            }
            send(tx, Bytes::copy_from_slice(&buffer[..read])).await?;
            left -= read as u64;
        }
        let padding = usize::try_from(tar::padding(entry.size)).unwrap_or(0);
        if padding > 0 {
            send(tx, Bytes::from(vec![0u8; padding])).await?;
        }
    }
    send(tx, Bytes::from_static(&tar::END)).await
}

/// Removes snapshots left behind by a server that stopped during a backup.
pub async fn remove_stale_snapshots(data_dir: &Path) {
    for (name, path, meta) in storage::list(data_dir).await {
        if meta.is_file() && name.starts_with(SNAPSHOT_PREFIX) && name.ends_with(".db") {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => tracing::info!(file = %name, "removed a stale backup snapshot"),
                Err(err) => {
                    tracing::warn!(file = %name, error = %err, "removing a stale backup snapshot failed")
                }
            }
        }
    }
}

/// File name for the download, e.g. `scrn-20261106-123456.tar`.
pub fn file_name(now: i64) -> String {
    let date = crate::html::format_date(now).replace('-', "");
    let secs = now.rem_euclid(86_400);
    format!(
        "scrn-{date}-{:02}{:02}{:02}.tar",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_file_name() {
        // 2026-11-06 12:34:56 UTC
        assert_eq!(file_name(1_793_968_496), "scrn-20261106-123456.tar");
        assert_eq!(file_name(0), "scrn-19700101-000000.tar");
    }

    #[test]
    fn names_in_the_archive() {
        let files = Path::new("/data/files");
        assert_eq!(
            archive_name(files, &files.join("3").join("a.png")),
            "files/3/a.png"
        );
        assert_eq!(archive_name(files, &files.join("a.png")), "files/a.png");
    }
}
