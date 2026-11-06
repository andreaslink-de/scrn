//! Screenshot files under `<data>/files/<user id>/`: `<id>.png` and
//! `<id>.thumb.png`. Both parts are numbers or fresh UUIDs from the server,
//! so a path can never be chosen by a client, and the directory names do
//! not reveal user names.

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
}
