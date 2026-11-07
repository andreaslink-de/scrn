//! Capture through the screenshot portal and cropping of the selection.

use std::path::{Path, PathBuf};

use ashpd::desktop::screenshot::Screenshot;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("screenshot portal: {0}")]
    Portal(#[from] ashpd::Error),
    #[error("the portal returned a URI that is no local file: {0}")]
    NotAFile(String),
    #[error("reading the screenshot failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("the screenshot is no PNG image: {0}")]
    Decode(#[from] png::DecodingError),
    #[error("writing the PNG failed: {0}")]
    Encode(#[from] png::EncodingError),
    #[error("unsupported PNG format {0:?}")]
    Format(png::ColorType),
    #[error("the selection lies outside the image")]
    Selection,
    #[error("the edited image is {0}x{1} px, expected {2}x{3} px")]
    Size(u32, u32, u32, u32),
}

impl CaptureError {
    /// The portal refused or failed, as opposed to reading the result.
    pub fn is_portal(&self) -> bool {
        matches!(self, Self::Portal(_))
    }

    /// The user closed a portal dialog.
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Portal(ashpd::Error::Response(
                ashpd::desktop::ResponseError::Cancelled
            ))
        )
    }
}

/// A captured screen: the PNG as the portal wrote it.
pub struct Capture {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Asks the portal for a screenshot of all screens without its own dialog,
/// reads it and removes the file the portal wrote.
pub async fn take() -> Result<(Capture, PathBuf), CaptureError> {
    let response = Screenshot::request()
        .interactive(false)
        .modal(true)
        .send()
        .await?
        .response()?;
    let uri = response.uri().as_str();
    let path = file_path(uri).ok_or_else(|| CaptureError::NotAFile(uri.to_owned()))?;
    let png = tokio::fs::read(&path).await?;
    // The portal of GNOME stores every screenshot in the pictures folder;
    // the client keeps only what the user selects.
    let _ = tokio::fs::remove_file(&path).await;
    let (width, height) = dimensions(&png)?;
    Ok((Capture { png, width, height }, path))
}

/// The local path of a `file://` URI, with percent escapes decoded.
fn file_path(uri: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    // Only local files: `file:///path` or `file://localhost/path`.
    let rest = uri.strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::with_capacity(path.len());
    let mut iter = path.bytes();
    while let Some(b) = iter.next() {
        if b == b'%' {
            let hex = [iter.next()?, iter.next()?];
            let value = u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?;
            bytes.push(value);
        } else {
            bytes.push(b);
        }
    }
    if bytes.contains(&0) {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

fn dimensions(data: &[u8]) -> Result<(u32, u32), CaptureError> {
    let reader = png::Decoder::new(std::io::Cursor::new(data)).read_info()?;
    let info = reader.info();
    Ok((info.width, info.height))
}

/// A rectangle in image pixels.
#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Crops `rect` out of the PNG and returns it as 8-bit RGB(A) PNG.
pub fn crop(data: &[u8], rect: Rect) -> Result<Vec<u8>, CaptureError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or(CaptureError::Selection)?];
    let frame = reader.next_frame(&mut buffer)?;
    let channels: usize = match frame.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => return Err(CaptureError::Format(other)),
    };
    let fits = rect.width > 0
        && rect.height > 0
        && rect
            .x
            .checked_add(rect.width)
            .is_some_and(|r| r <= frame.width)
        && rect
            .y
            .checked_add(rect.height)
            .is_some_and(|b| b <= frame.height);
    if !fits {
        return Err(CaptureError::Selection);
    }
    let (x, y, w, h) = (
        rect.x as usize,
        rect.y as usize,
        rect.width as usize,
        rect.height as usize,
    );
    let mut pixels = Vec::with_capacity(w * h * channels);
    for row in y..y + h {
        let start = row * frame.line_size + x * channels;
        pixels.extend_from_slice(&buffer[start..start + w * channels]);
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, rect.width, rect.height);
        encoder.set_color(frame.color_type);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&pixels)?;
    }
    Ok(out)
}

/// Checks that the editor returned a PNG of the size it was given. The
/// editor runs in the webview, so its output is decoded before use.
pub fn check_edit(data: &[u8], width: u32, height: u32) -> Result<(), CaptureError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let (w, h) = (reader.info().width, reader.info().height);
    if (w, h) != (width, height) {
        return Err(CaptureError::Size(w, h, width, height));
    }
    let mut buffer = vec![0; reader.output_buffer_size().ok_or(CaptureError::Selection)?];
    reader.next_frame(&mut buffer)?;
    Ok(())
}

/// Saves a finished image as `scrn-<unix millis>.png` in [`output_dir`].
pub fn save(png: &[u8]) -> Result<PathBuf, CaptureError> {
    let dir = output_dir();
    std::fs::create_dir_all(&dir)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = dir.join(format!("scrn-{millis}.png"));
    std::fs::write(&path, png)?;
    Ok(path)
}

/// `~/Pictures/SCRN` (or `$XDG_PICTURES_DIR/SCRN` when set).
pub fn output_dir() -> PathBuf {
    let pictures = std::env::var_os("XDG_PICTURES_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join("Pictures")))
        .unwrap_or_else(std::env::temp_dir);
    pictures.join("SCRN")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[x as u8, y as u8, 7]);
            }
        }
        writer.write_image_data(&data).unwrap();
        drop(writer);
        out
    }

    #[test]
    fn file_uris_become_paths() {
        assert_eq!(
            file_path("file:///home/a/Bilder/Bildschirmfoto%20vom%202026-11-06%20%C3%A4.png"),
            Some(PathBuf::from(
                "/home/a/Bilder/Bildschirmfoto vom 2026-11-06 ä.png"
            ))
        );
        assert_eq!(
            file_path("file://localhost/tmp/x.png"),
            Some(PathBuf::from("/tmp/x.png"))
        );
        for bad in [
            "https://x/y.png",
            "file://host/x.png",
            "file:///a%2",
            "file:///a%zz",
            "file:///a%00b",
        ] {
            assert_eq!(file_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn crops_the_selected_pixels() {
        let source = image(10, 8);
        let rect = Rect {
            x: 2,
            y: 3,
            width: 4,
            height: 2,
        };
        let out = crop(&source, rect).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(&out[..]))
            .read_info()
            .unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut buffer).unwrap();
        assert_eq!((frame.width, frame.height), (4, 2));
        assert_eq!(&buffer[..3], &[2, 3, 7]);
        let last = &buffer[frame.line_size + 3 * 3..frame.line_size + 4 * 3];
        assert_eq!(last, &[5, 4, 7]);
        assert_eq!(dimensions(&out).unwrap(), (4, 2));
    }

    #[test]
    fn edits_must_keep_the_size() {
        let edit = image(6, 4);
        assert!(check_edit(&edit, 6, 4).is_ok());
        assert!(matches!(
            check_edit(&edit, 6, 5),
            Err(CaptureError::Size(6, 4, 6, 5))
        ));
        assert!(matches!(
            check_edit(b"not a png", 6, 4),
            Err(CaptureError::Decode(_))
        ));
        // A truncated PNG has a valid header but no complete image data.
        assert!(check_edit(&edit[..edit.len() - 20], 6, 4).is_err());
    }

    #[test]
    fn selections_outside_the_image_are_rejected() {
        let source = image(10, 8);
        for rect in [
            Rect {
                x: 8,
                y: 0,
                width: 3,
                height: 1,
            },
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            Rect {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1,
            },
        ] {
            assert!(matches!(crop(&source, rect), Err(CaptureError::Selection)));
        }
    }
}
