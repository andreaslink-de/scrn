//! Screenshot files under `<data>/files`: `<id>.png` and `<id>.thumb.png`.
//! The IDs are fresh UUIDs, so a file name can never be chosen by a client.

use std::io;
use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub fn image_path(files_dir: &Path, id: Uuid) -> PathBuf {
    files_dir.join(format!("{id}.png"))
}

pub fn thumb_path(files_dir: &Path, id: Uuid) -> PathBuf {
    files_dir.join(format!("{id}.thumb.png"))
}

/// Writes the image and its thumbnail. Existing files are never replaced.
/// On error, the files this call created are removed again.
pub async fn write(files_dir: &Path, id: Uuid, image: &[u8], thumb: &[u8]) -> io::Result<()> {
    let image_path = image_path(files_dir, id);
    write_new(&image_path, image).await?;
    if let Err(err) = write_new(&thumb_path(files_dir, id), thumb).await {
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
pub async fn remove(files_dir: &Path, id: Uuid) -> io::Result<()> {
    let mut first_error = None;
    for path in [image_path(files_dir, id), thumb_path(files_dir, id)] {
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
        let id = Uuid::new_v4();
        write(dir.path(), id, b"image", b"thumb").await.unwrap();
        assert_eq!(std::fs::read(image_path(dir.path(), id)).unwrap(), b"image");
        assert_eq!(std::fs::read(thumb_path(dir.path(), id)).unwrap(), b"thumb");

        // Existing files are never overwritten, and a failed write leaves
        // them alone.
        assert!(write(dir.path(), id, b"other", b"other").await.is_err());
        assert_eq!(std::fs::read(image_path(dir.path(), id)).unwrap(), b"image");

        // If the thumbnail cannot be written, the new image is removed.
        let other = Uuid::new_v4();
        std::fs::write(thumb_path(dir.path(), other), b"blocker").unwrap();
        assert!(write(dir.path(), other, b"image", b"thumb").await.is_err());
        assert!(!image_path(dir.path(), other).exists());

        remove(dir.path(), id).await.unwrap();
        assert!(!image_path(dir.path(), id).exists());
        assert!(!thumb_path(dir.path(), id).exists());
        // Removing again is fine.
        remove(dir.path(), id).await.unwrap();
    }
}
