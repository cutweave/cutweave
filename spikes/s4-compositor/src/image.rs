//! Output images (RGBA, 8 or 16 bits per channel) and PNG I/O.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use crate::scene::{OutputTransform, Primaries};
use crate::{Error, Result};

/// Interleaved RGBA image; samples are stored as `u16` for both bit depths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub data: Vec<u16>,
}

impl Image {
    /// Largest code value.
    #[must_use]
    pub fn max_value(&self) -> u16 {
        if self.bit_depth == 16 { u16::MAX } else { 255 }
    }

    /// RGBA of pixel (x, y).
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> [u16; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Count of differing samples (0 ⇒ bit-identical).
    #[must_use]
    pub fn diff_count(&self, other: &Self) -> usize {
        if self.width != other.width || self.height != other.height {
            return usize::MAX;
        }
        self.data
            .iter()
            .zip(&other.data)
            .filter(|(a, b)| a != b)
            .count()
    }

    /// Largest absolute per-sample difference.
    #[must_use]
    pub fn max_abs_diff(&self, other: &Self) -> u16 {
        self.data
            .iter()
            .zip(&other.data)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0)
    }

    /// Amplified absolute-difference visualisation (8-bit, opaque).
    #[must_use]
    pub fn diff_image(&self, other: &Self, gain: u32) -> Self {
        let scale = u32::from(self.max_value() / 255).max(1);
        let mut data = Vec::with_capacity(self.data.len());
        for (i, (a, b)) in self.data.iter().zip(&other.data).enumerate() {
            if i % 4 == 3 {
                data.push(255);
            } else {
                data.push((u32::from(a.abs_diff(*b)) / scale * gain).min(255) as u16);
            }
        }
        Self {
            width: self.width,
            height: self.height,
            bit_depth: 8,
            data,
        }
    }
}

/// `cICP` code points `[primaries, transfer, matrix, full_range]` for an output transform.
#[must_use]
pub fn cicp_for(out: &OutputTransform) -> [u8; 4] {
    let prim = match out.primaries {
        Primaries::Bt709 => Primaries::Bt709.h273(),
        Primaries::Bt2020 => Primaries::Bt2020.h273(),
    };
    [prim, out.transfer.h273(), 0, 1]
}

fn png_err(e: impl std::fmt::Display) -> Error {
    Error::Image(e.to_string())
}

/// Encodes an image to PNG bytes, with an optional `cICP` chunk describing its encoding.
pub fn encode_png(img: &Image, cicp: Option<[u8; 4]>) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(if img.bit_depth == 16 {
            png::BitDepth::Sixteen
        } else {
            png::BitDepth::Eight
        });
        let mut w = enc.write_header().map_err(png_err)?;
        if let Some(c) = cicp {
            w.write_chunk(png::chunk::cICP, &c).map_err(png_err)?;
        }
        let bytes: Vec<u8> = if img.bit_depth == 16 {
            img.data.iter().flat_map(|v| v.to_be_bytes()).collect()
        } else {
            img.data.iter().map(|&v| v.min(255) as u8).collect()
        };
        w.write_image_data(&bytes).map_err(png_err)?;
        w.finish().map_err(png_err)?;
    }
    Ok(out)
}

/// Writes a PNG file.
pub fn write_png(path: &Path, img: &Image, cicp: Option<[u8; 4]>) -> Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = encode_png(img, cicp)?;
    let mut f = BufWriter::new(File::create(path)?);
    std::io::Write::write_all(&mut f, &bytes)?;
    Ok(())
}

/// Reads an RGB/RGBA PNG (8 or 16 bit) into RGBA.
pub fn read_png(path: &Path) -> Result<Image> {
    let dec = png::Decoder::new(BufReader::new(File::open(path)?));
    let mut reader = dec.read_info().map_err(png_err)?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| png_err("image too large"))?;
    let mut buf = vec![0u8; size];
    let info = reader.next_frame(&mut buf).map_err(png_err)?;
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        other => return Err(png_err(format!("unsupported PNG colour type {other:?}"))),
    };
    let sixteen = match info.bit_depth {
        png::BitDepth::Eight => false,
        png::BitDepth::Sixteen => true,
        other => return Err(png_err(format!("unsupported PNG bit depth {other:?}"))),
    };
    let bytes = &buf[..info.buffer_size()];
    let samples: Vec<u16> = if sixteen {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect()
    } else {
        bytes.iter().map(|&b| u16::from(b)).collect()
    };
    let opaque = if sixteen { u16::MAX } else { 255 };
    let data = if channels == 4 {
        samples
    } else {
        samples
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[1], c[2], opaque])
            .collect()
    };
    Ok(Image {
        width: info.width,
        height: info.height,
        bit_depth: if sixteen { 16 } else { 8 },
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip_8_and_16_bit() {
        let dir = std::env::temp_dir().join(format!("s4-png-{}", std::process::id()));
        for bits in [8u8, 16] {
            let max = if bits == 16 { 65535u32 } else { 255 };
            let data = (0..4 * 6 * 3)
                .map(|i| ((i * 977) as u32 % (max + 1)) as u16)
                .collect();
            let img = Image {
                width: 6,
                height: 3,
                bit_depth: bits,
                data,
            };
            let p = dir.join(format!("t{bits}.png"));
            write_png(&p, &img, Some([1, 1, 0, 1])).expect("write");
            assert_eq!(read_png(&p).expect("read"), img);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
