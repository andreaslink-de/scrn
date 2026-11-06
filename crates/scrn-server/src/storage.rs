//! Screenshot files under `<data>/files/<user id>/`: `<id>.png` and
//! `<id>.thumb.png`. Both parts are numbers or fresh UUIDs from the server,
//! so a path can never be chosen by a client, and the directory names do
//! not reveal user names.

use std::fs::Metadata;
use std::io;
use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// The directory of all files of a user.
pub fn user_dir(files_dir: &Path, user_id: i64) -> PathBuf {
    files_dir.join(user_id.to_string())
}

pub fn image_path(files_dir: &Path, user_id: i64, id: Uuid) -> PathBuf {
    user_dir(files_dir, user_id).join(format!("{id}.png"))
}

pub fn thumb_path(files_dir: &Path, user_id: i64, id: Uuid) -> PathBuf {
    user_dir(files_dir, user_id).join(format!("{id}.thumb.png"))
}

/// Writes the image and its thumbnail, creating the user directory if
/// needed. Existing files are never replaced. On error, the files this call
/// created are removed again.
pub async fn write(
    files_dir: &Path,
    user_id: i64,
    id: Uuid,
    image: &[u8],
    thumb: &[u8],
) -> io::Result<()> {
    tokio::fs::create_dir_all(user_dir(files_dir, user_id)).await?;
    let image_path = image_path(files_dir, user_id, id);
    write_new(&image_path, image).await?;
    if let Err(err) = write_new(&thumb_path(files_dir, user_id, id), thumb).await {
        let _ = tokio::fs::remove_file(&image_path).await;
        return Err(err);
    }
    Ok(())
}

/// Creates `path` and writes `data`. A partly written file is removed.
async fn write_new(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    let result = async {
        file.write_all(data).await?;
        file.sync_all().await
    }
    .await;
    if result.is_err() {
        drop(file);
        let _ = tokio::fs::remove_file(path).await;
    }
    result
}

/// Removes the image and its thumbnail. Missing files are not an error.
pub async fn remove(files_dir: &Path, user_id: i64, id: Uuid) -> io::Result<()> {
    let mut first_error = None;
    for path in [
        image_path(files_dir, user_id, id),
        thumb_path(files_dir, user_id, id),
    ] {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                first_error.get_or_insert(err);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// The regular files in `dir` whose names we write, with their IDs.
pub async fn screenshot_files(dir: &Path) -> Vec<(Uuid, PathBuf, Metadata)> {
    let mut files = Vec::new();
    for (name, path, meta) in list(dir).await {
        if let Some(id) = screenshot_id(&name).filter(|_| meta.is_file()) {
            files.push((id, path, meta));
        }
    }
    files
}

/// The user directories in `dir`: subdirectories named by a user ID.
pub async fn user_dirs(dir: &Path) -> Vec<(i64, PathBuf)> {
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
pub async fn list(dir: &Path) -> Vec<(String, PathBuf, Metadata)> {
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
pub fn screenshot_id(name: &str) -> Option<Uuid> {
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

    #[tokio::test]
    async fn write_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let id = Uuid::new_v4();
        write(d, 7, id, b"image", b"thumb").await.unwrap();
        assert_eq!(image_path(d, 7, id), d.join("7").join(format!("{id}.png")));
        assert_eq!(std::fs::read(image_path(d, 7, id)).unwrap(), b"image");
        assert_eq!(std::fs::read(thumb_path(d, 7, id)).unwrap(), b"thumb");

        // Existing files are never overwritten, and a failed write leaves
        // them alone.
        assert!(write(d, 7, id, b"other", b"other").await.is_err());
        assert_eq!(std::fs::read(image_path(d, 7, id)).unwrap(), b"image");

        // If the thumbnail cannot be written, the new image is removed.
        let other = Uuid::new_v4();
        std::fs::write(thumb_path(d, 7, other), b"blocker").unwrap();
        assert!(write(d, 7, other, b"image", b"thumb").await.is_err());
        assert!(!image_path(d, 7, other).exists());

        remove(d, 7, id).await.unwrap();
        assert!(!image_path(d, 7, id).exists());
        assert!(!thumb_path(d, 7, id).exists());
        // Removing again is fine.
        remove(d, 7, id).await.unwrap();
    }

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
