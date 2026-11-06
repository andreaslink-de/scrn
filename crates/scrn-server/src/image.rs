//! PNG validation, re-encoding and thumbnails.
//!
//! Uploads are decoded with resource limits and written again from the raw
//! pixels. The new file contains only the image header and pixel data, so
//! every ancillary chunk (text, EXIF, ICC profiles, timestamps) of the
//! original is dropped. Animated PNGs keep only their first frame.

use std::io::Cursor;

use png::{BitDepth, ColorType, Compression, DecodingError, Transformations};

/// Upper bound of width × height.
pub const MAX_PIXELS: u64 = 100_000_000;
/// Thumbnails are at most this wide ...
pub const THUMB_MAX_WIDTH: u32 = 480;
/// ... and at most this tall, so very tall screenshots stay small too.
pub const THUMB_MAX_HEIGHT: u32 = 1440;
/// Memory the decoder may use for its own buffers (chunks, rows). The pixel
/// buffer is checked separately through [`MAX_PIXELS`].
const DECODER_MEMORY: usize = 32 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ImageError {
    #[error("the data is not a valid PNG image")]
    Invalid,
    #[error("the image exceeds the size limits")]
    TooLarge,
    #[error("encoding the image failed")]
    Encode,
}

/// A validated, re-encoded image with its thumbnail.
#[derive(Debug)]
pub struct Processed {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub thumb: Vec<u8>,
}

/// Decoded 8-bit pixels.
struct Pixels {
    data: Vec<u8>,
    width: u32,
    height: u32,
    color: ColorType,
}

/// Validates `data` as PNG and returns the re-encoded image and its
/// thumbnail. CPU and memory heavy: call it from a blocking task.
pub fn process(data: &[u8]) -> Result<Processed, ImageError> {
    let pixels = decode(data)?;
    let png = encode(&pixels)?;
    let (tw, th) = thumb_size(pixels.width, pixels.height);
    let thumb = if (tw, th) == (pixels.width, pixels.height) {
        png.clone()
    } else {
        encode(&downscale(&pixels, tw, th))?
    };
    Ok(Processed {
        png,
        width: pixels.width,
        height: pixels.height,
        thumb,
    })
}

fn decode(data: &[u8]) -> Result<Pixels, ImageError> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(data),
        png::Limits {
            bytes: DECODER_MEMORY,
        },
    );
    // Expand palettes and low bit depths, strip 16-bit to 8-bit.
    decoder.set_transformations(Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(map_decoding_error)?;
    let info = reader.info();
    if u64::from(info.width) * u64::from(info.height) > MAX_PIXELS {
        return Err(ImageError::TooLarge);
    }
    let size = reader.output_buffer_size().ok_or(ImageError::TooLarge)?;
    let mut buf = vec![0; size];
    let frame = reader.next_frame(&mut buf).map_err(map_decoding_error)?;
    if frame.bit_depth != BitDepth::Eight || frame.width == 0 || frame.height == 0 {
        return Err(ImageError::Invalid);
    }
    let channels = channels(frame.color_type).ok_or(ImageError::Invalid)?;
    let line = usize::try_from(frame.width).map_err(|_| ImageError::TooLarge)? * channels;
    if frame.line_size != line {
        return Err(ImageError::Invalid);
    }
    let len = line * usize::try_from(frame.height).map_err(|_| ImageError::TooLarge)?;
    if buf.len() < len {
        return Err(ImageError::Invalid);
    }
    buf.truncate(len);
    Ok(Pixels {
        data: buf,
        width: frame.width,
        height: frame.height,
        color: frame.color_type,
    })
}

fn map_decoding_error(err: DecodingError) -> ImageError {
    match err {
        DecodingError::LimitsExceeded => ImageError::TooLarge,
        _ => ImageError::Invalid,
    }
}

fn channels(color: ColorType) -> Option<usize> {
    match color {
        ColorType::Grayscale => Some(1),
        ColorType::GrayscaleAlpha => Some(2),
        ColorType::Rgb => Some(3),
        ColorType::Rgba => Some(4),
        ColorType::Indexed => None,
    }
}

fn has_alpha(color: ColorType) -> bool {
    matches!(color, ColorType::GrayscaleAlpha | ColorType::Rgba)
}

fn encode(pixels: &Pixels) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, pixels.width, pixels.height);
    encoder.set_color(pixels.color);
    encoder.set_depth(BitDepth::Eight);
    encoder.set_compression(Compression::Balanced);
    let mut writer = encoder.write_header().map_err(|_| ImageError::Encode)?;
    writer
        .write_image_data(&pixels.data)
        .map_err(|_| ImageError::Encode)?;
    writer.finish().map_err(|_| ImageError::Encode)?;
    Ok(out)
}

/// Size of the thumbnail: fits into the maximum box, keeps the aspect ratio
/// and never enlarges.
fn thumb_size(width: u32, height: u32) -> (u32, u32) {
    if width <= THUMB_MAX_WIDTH && height <= THUMB_MAX_HEIGHT {
        return (width, height);
    }
    let (w, h) = (u64::from(width), u64::from(height));
    let (max_w, max_h) = (u64::from(THUMB_MAX_WIDTH), u64::from(THUMB_MAX_HEIGHT));
    let (tw, th) = if w * max_h >= h * max_w {
        (max_w, (h * max_w / w).max(1))
    } else {
        ((w * max_h / h).max(1), max_h)
    };
    // Both values are bounded by the maximum box.
    (
        u32::try_from(tw).unwrap_or(THUMB_MAX_WIDTH),
        u32::try_from(th).unwrap_or(THUMB_MAX_HEIGHT),
    )
}

/// Box filter: every target pixel is the average of the source pixels that
/// map onto it. Colors are weighted by alpha, so fully transparent pixels do
/// not bleed their (invisible) color into the result.
fn downscale(src: &Pixels, tw: u32, th: u32) -> Pixels {
    let ch = channels(src.color).unwrap_or(4);
    let alpha = has_alpha(src.color);
    let (sw, sh) = (src.width as usize, src.height as usize);
    let (tw_us, th_us) = (tw as usize, th as usize);
    let xmap: Vec<usize> = (0..sw).map(|x| x * tw_us / sw).collect();

    let mut sums = vec![0u64; tw_us * th_us * ch];
    let mut counts = vec![0u64; tw_us * th_us];
    for (y, row) in src.data.chunks_exact(sw * ch).enumerate() {
        let ty = y * th_us / sh;
        for (x, px) in row.chunks_exact(ch).enumerate() {
            let t = ty * tw_us + xmap[x];
            counts[t] += 1;
            let sum = &mut sums[t * ch..(t + 1) * ch];
            if alpha {
                let a = u64::from(px[ch - 1]);
                for c in 0..ch - 1 {
                    sum[c] += u64::from(px[c]) * a;
                }
                sum[ch - 1] += a;
            } else {
                for c in 0..ch {
                    sum[c] += u64::from(px[c]);
                }
            }
        }
    }

    let mut data = Vec::with_capacity(tw_us * th_us * ch);
    for (t, &n) in counts.iter().enumerate() {
        let sum = &sums[t * ch..(t + 1) * ch];
        let n = n.max(1);
        if alpha {
            let a = sum[ch - 1];
            for &c in &sum[..ch - 1] {
                data.push((c + a / 2).checked_div(a).map_or(0, to_u8));
            }
            data.push(to_u8((a + n / 2) / n));
        } else {
            for &c in sum {
                data.push(to_u8((c + n / 2) / n));
            }
        }
    }
    Pixels {
        data,
        width: tw,
        height: th,
        color: src.color,
    }
}

fn to_u8(v: u64) -> u8 {
    u8::try_from(v).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(width: u32, height: u32, color: ColorType, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(BitDepth::Eight);
        encoder
            .add_text_chunk("Author".into(), "secret-author".into())
            .unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(data).unwrap();
        writer.finish().unwrap();
        out
    }

    fn decode_test(data: &[u8]) -> Pixels {
        decode(data).unwrap()
    }

    #[test]
    fn keeps_pixels_and_drops_metadata() {
        let data: Vec<u8> = (0..4 * 3 * 2)
            .map(|i| u8::try_from(i * 10).unwrap())
            .collect();
        let original = png_bytes(4, 2, ColorType::Rgb, &data);
        assert!(original.windows(13).any(|w| w == b"secret-author"));

        let out = process(&original).unwrap();
        assert_eq!((out.width, out.height), (4, 2));
        assert!(!out.png.windows(13).any(|w| w == b"secret-author"));
        assert!(!out.png.windows(4).any(|w| w == b"tEXt"));
        let pixels = decode_test(&out.png);
        assert_eq!(pixels.data, data);
        // Small images are their own thumbnail.
        assert_eq!(out.thumb, out.png);
    }

    #[test]
    fn rejects_non_png_data() {
        assert_eq!(process(b"hello world").unwrap_err(), ImageError::Invalid);
        assert_eq!(process(b"").unwrap_err(), ImageError::Invalid);
        // A valid file cut short.
        let png = png_bytes(4, 4, ColorType::Rgb, &[7; 48]);
        assert_eq!(
            process(&png[..png.len() - 20]).unwrap_err(),
            ImageError::Invalid
        );
    }

    #[test]
    fn rejects_too_many_pixels_before_decoding() {
        // 20000 × 20000 = 400 megapixels; only the header is read.
        let png = png_header_only(20_000, 20_000);
        assert_eq!(process(&png).unwrap_err(), ImageError::TooLarge);
    }

    /// A PNG signature, an IHDR chunk and an empty IDAT chunk.
    pub(crate) fn png_header_only(width: u32, height: u32) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        for (kind, body) in [(&b"IHDR"[..], ihdr.as_slice()), (b"IDAT", &[])] {
            out.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
            let mut chunk = kind.to_vec();
            chunk.extend_from_slice(body);
            out.extend_from_slice(&chunk);
            out.extend_from_slice(&crc32fast_compatible(&chunk).to_be_bytes());
        }
        out
    }

    fn crc32fast_compatible(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn palette_and_16_bit_images_are_normalized() {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, 2, 1);
        encoder.set_color(ColorType::Rgb);
        encoder.set_depth(BitDepth::Sixteen);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[0xff, 0xff, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0xff, 0xff])
            .unwrap();
        writer.finish().unwrap();
        let processed = process(&out).unwrap();
        let pixels = decode_test(&processed.png);
        assert_eq!(pixels.color, ColorType::Rgb);
        assert_eq!(pixels.data, [0xff, 0, 0, 0, 0x80, 0xff]);
    }

    #[test]
    fn thumbnail_sizes() {
        assert_eq!(thumb_size(400, 300), (400, 300));
        assert_eq!(thumb_size(1920, 1080), (480, 270));
        assert_eq!(thumb_size(960, 4000), (345, 1440));
        assert_eq!(thumb_size(100_000, 1), (480, 1));
        assert_eq!(thumb_size(1, 100_000), (1, 1440));
    }

    #[test]
    fn large_images_get_a_downscaled_thumbnail() {
        let (w, h) = (960u32, 20u32);
        // Left half black, right half white.
        let data: Vec<u8> = (0..h)
            .flat_map(|_| (0..w).map(|x| if x < w / 2 { 0 } else { 255 }))
            .collect();
        let processed = process(&png_bytes(w, h, ColorType::Grayscale, &data)).unwrap();
        assert_eq!((processed.width, processed.height), (960, 20));
        let thumb = decode_test(&processed.thumb);
        assert_eq!((thumb.width, thumb.height), (480, 10));
        assert_eq!(thumb.data[0], 0);
        assert_eq!(thumb.data[479], 255);
    }

    #[test]
    fn transparent_pixels_do_not_bleed_color() {
        // One opaque red pixel next to a transparent green one.
        let src = Pixels {
            data: vec![255, 0, 0, 255, 0, 255, 0, 0],
            width: 2,
            height: 1,
            color: ColorType::Rgba,
        };
        let out = downscale(&src, 1, 1);
        assert_eq!(out.data, [255, 0, 0, 128]);
    }
}
