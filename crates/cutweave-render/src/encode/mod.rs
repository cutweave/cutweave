// SPDX-License-Identifier: GPL-3.0-or-later
//! Encoded images for agents (arch §14 "seeing without screenshots"): JPEG from the in-crate
//! baseline encoder ([`encode_jpeg`]), PNG through the `png` crate ([`encode_png`]), base64 for
//! JSON transport, and the `max_size` fit.
//!
//! Both encoders are deterministic: the same pixels and settings give the same bytes.

mod jpeg;

use cutweave_commands::wire;

pub use jpeg::encode_jpeg;

use crate::image::Image;

/// JPEG quality of agent frames when none is given (WP-1F.03).
pub const DEFAULT_JPEG_QUALITY: u8 = 80;

/// Longest image side of agent frames when no `max_size` is given (arch §14).
pub const DEFAULT_MAX_SIZE: u32 = 768;

/// H.273 `cICP` code points of PNG output: BT.709 primaries, sRGB transfer, RGB, full range.
const CICP_SRGB: [u8; 4] = [1, 13, 0, 1];

/// An image file format.
#[wire]
#[derive(Copy, Default, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    /// Baseline JPEG, 4:2:0 (alpha flattened over black).
    #[default]
    Jpeg,
    /// Lossless PNG (RGB when opaque, else RGBA).
    Png,
}

impl ImageFormat {
    /// The MIME type.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
        }
    }
}

/// Why an image could not be encoded.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// The image has no pixels or its buffer does not match its size.
    #[error("cannot encode a {width}x{height} image with a mismatched or empty buffer")]
    InvalidImage {
        /// Width.
        width: u32,
        /// Height.
        height: u32,
    },
    /// The format cannot hold an image this large.
    #[error("{width}x{height} exceeds the format limit of {max} pixels per side")]
    TooLarge {
        /// Width.
        width: u32,
        /// Height.
        height: u32,
        /// Largest side the format allows.
        max: u32,
    },
    /// The PNG encoder failed.
    #[error("PNG encoding failed: {0}")]
    Png(String),
}

/// Encodes `image` as a PNG into `out` (cleared first). Opaque images are written as RGB.
pub fn encode_png(image: &Image, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    if image.width == 0 || image.height == 0 || !image.is_valid() {
        return Err(EncodeError::InvalidImage {
            width: image.width,
            height: image.height,
        });
    }
    let png_err = |e: png::EncodingError| EncodeError::Png(e.to_string());
    let opaque = image.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
    out.clear();
    let mut enc = png::Encoder::new(&mut *out, image.width, image.height);
    enc.set_color(if opaque {
        png::ColorType::Rgb
    } else {
        png::ColorType::Rgba
    });
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut w = enc.write_header().map_err(png_err)?;
    w.write_chunk(png::chunk::cICP, &CICP_SRGB)
        .map_err(png_err)?;
    if opaque {
        let rgb: Vec<u8> = image
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        w.write_image_data(&rgb).map_err(png_err)?;
    } else {
        w.write_image_data(&image.data).map_err(png_err)?;
    }
    w.finish().map_err(png_err)
}

/// Encodes `image` in `format` into `out` (cleared first); `jpeg_quality` (1–100) applies to
/// JPEG only.
pub fn encode(
    image: &Image,
    format: ImageFormat,
    jpeg_quality: u8,
    out: &mut Vec<u8>,
) -> Result<(), EncodeError> {
    match format {
        ImageFormat::Jpeg => encode_jpeg(image, jpeg_quality, out),
        ImageFormat::Png => encode_png(image, out),
    }
}

/// Standard base64 (RFC 4648 §4, padded): how image bytes travel in JSON results.
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `size` scaled (aspect kept, rounded) so its longer side is at most `max`; never enlarged;
/// at least 1×1.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn fit_within((width, height): (u32, u32), max: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= max || long == 0 {
        return (width.max(1), height.max(1));
    }
    let max = max.max(1);
    let scale =
        |v: u32| ((u64::from(v) * u64::from(max) + u64::from(long) / 2) / u64::from(long)) as u32;
    (scale(width).max(1), scale(height).max(1))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), want, "{input}");
        }
        assert_eq!(base64(&[0xFF, 0xFE, 0xFD]), "//79");
    }

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit_within((1920, 1080), 768), (768, 432));
        assert_eq!(fit_within((1080, 1920), 768), (432, 768));
        assert_eq!(fit_within((640, 360), 768), (640, 360));
        assert_eq!(fit_within((1920, 1080), 1920), (1920, 1080));
        assert_eq!(fit_within((4000, 3), 16), (16, 1));
        assert_eq!(fit_within((0, 0), 16), (1, 1));
        assert_eq!(fit_within((1000, 1000), 0), (1, 1));
    }

    #[test]
    fn png_round_trips_and_drops_alpha_when_opaque() {
        let mut img = Image::filled(7, 3, [10, 20, 30, 255]);
        img.set_pixel(2, 1, [250, 0, 5, 255]);
        let mut out = Vec::new();
        encode_png(&img, &mut out).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(&out));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgb);
        assert_eq!((info.width, info.height), (7, 3));
        assert_eq!(&buf[(7 + 2) * 3..(7 + 2) * 3 + 3], &[250, 0, 5]);

        img.set_pixel(0, 0, [1, 2, 3, 128]);
        encode_png(&img, &mut out).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(&out))
            .read_info()
            .unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(&buf[..info.buffer_size()], &img.data[..]);
        assert_eq!(ImageFormat::Png.mime(), "image/png");
    }

    #[test]
    fn png_rejects_mismatched_buffers() {
        let bad = Image {
            width: 4,
            height: 4,
            data: vec![0; 3],
        };
        assert!(matches!(
            encode(&bad, ImageFormat::Png, 80, &mut Vec::new()),
            Err(EncodeError::InvalidImage { .. })
        ));
    }
}
