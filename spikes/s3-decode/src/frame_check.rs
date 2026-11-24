// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-frame identity checks on the decoded luma plane: CRC-32 of the Y plane (compared with
//! `ffmpeg -vf extractplanes=y -f framehash -hash crc32`) and the burned-in frame-number barcode
//! written by `gen_media.py`.

use ffmpeg_next as ffmpeg;

use crate::ffi;
use crate::util::Crc32;

/// Borrowed view of a planar luma plane.
pub struct Luma<'a> {
    pub data: &'a [u8],
    pub stride: usize,
    pub width: usize,
    pub height: usize,
    /// Bytes per sample (1 for 8-bit, 2 for 9..16-bit little-endian).
    pub bytes: usize,
    pub depth: u32,
}

impl<'a> Luma<'a> {
    /// The Y plane of a decoded frame; `None` for packed or unknown formats.
    pub fn of(frame: &'a ffmpeg::frame::Video) -> Option<Self> {
        let (depth, bytes, planar) = ffi::luma_layout(frame.format())?;
        if !planar || frame.planes() == 0 {
            return None;
        }
        Some(Luma {
            data: frame.data(0),
            stride: frame.stride(0),
            width: frame.width() as usize,
            height: frame.height() as usize,
            bytes,
            depth,
        })
    }

    /// One row without stride padding.
    pub fn row(&self, y: usize) -> &'a [u8] {
        let start = y * self.stride;
        &self.data[start..start + self.width * self.bytes]
    }

    /// Sample value at (x, y).
    pub fn sample(&self, x: usize, y: usize) -> u32 {
        let o = y * self.stride + x * self.bytes;
        if self.bytes == 1 {
            u32::from(self.data[o])
        } else {
            u32::from(u16::from_le_bytes([self.data[o], self.data[o + 1]]))
        }
    }

    /// CRC-32 over the rows (what `extractplanes=y` + rawvideo + framehash hashes).
    pub fn crc(&self, crc: &Crc32) -> u32 {
        let mut s = 0xFFFF_FFFF;
        for y in 0..self.height {
            s = crc.update(s, self.row(y));
        }
        s ^ 0xFFFF_FFFF
    }

    /// Mean of a small window centred on (cx, cy).
    fn window_mean(&self, cx: usize, cy: usize, rx: usize, ry: usize) -> f64 {
        let (x0, x1) = (cx.saturating_sub(rx), (cx + rx).min(self.width - 1));
        let (y0, y1) = (cy.saturating_sub(ry), (cy + ry).min(self.height - 1));
        let mut sum = 0u64;
        let mut n = 0u64;
        for y in y0..=y1 {
            for x in x0..=x1 {
                sum += u64::from(self.sample(x, y));
                n += 1;
            }
        }
        sum as f64 / n.max(1) as f64
    }

    /// Reads the 16-bit frame-number barcode: bit k is set when the block in band 0 is brighter
    /// than the block below it in band 1. `None` when any bit is ambiguous (no barcode).
    pub fn barcode(&self) -> Option<u32> {
        const BITS: usize = 16;
        const BANDS: usize = 18;
        if self.width < BITS * 4 || self.height < BANDS * 2 {
            return None;
        }
        let (bw, bh) = (self.width / BITS, self.height / BANDS);
        let full = f64::from((1u32 << self.depth) - 1);
        let mut value = 0u32;
        for k in 0..BITS {
            let cx = k * bw + bw / 2;
            let top = self.window_mean(cx, bh / 2, bw / 6, bh / 6);
            let bottom = self.window_mean(cx, bh + bh / 2, bw / 6, bh / 6);
            if (top - bottom).abs() < 0.25 * full {
                return None;
            }
            if top > bottom {
                value |= 1 << k;
            }
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws the generator's layout into a buffer and reads it back.
    fn synth(w: usize, h: usize, n: u32, bytes: usize, depth: u32) -> Vec<u8> {
        let stride = w * bytes + 32;
        let mut buf = vec![0u8; stride * h];
        let (lo, hi) = (16u32 << (depth - 8), 235u32 << (depth - 8));
        let (bw, bh) = (w / 16, h / 18);
        for y in 0..h {
            for x in 0..w {
                let k = x / bw;
                let v = if k < 16 && y < 2 * bh {
                    let bit = (n >> k) & 1 == 1;
                    let band0 = y < bh;
                    if band0 == bit { hi } else { lo }
                } else {
                    (x as u32 * 7 + y as u32) % 200 + 20
                };
                let o = y * stride + x * bytes;
                if bytes == 1 {
                    buf[o] = v as u8;
                } else {
                    buf[o..o + 2].copy_from_slice(&(v as u16).to_le_bytes());
                }
            }
        }
        buf
    }

    #[test]
    fn barcode_round_trips_8_and_10_bit() {
        for (bytes, depth) in [(1, 8), (2, 10)] {
            for n in [0u32, 1, 2, 299, 4095, 65535] {
                let (w, h) = (640, 360);
                let buf = synth(w, h, n, bytes, depth);
                let l = Luma {
                    data: &buf,
                    stride: w * bytes + 32,
                    width: w,
                    height: h,
                    bytes,
                    depth,
                };
                assert_eq!(l.barcode(), Some(n), "n={n} depth={depth}");
            }
        }
    }

    #[test]
    fn flat_plane_has_no_barcode() {
        let buf = vec![128u8; 640 * 360];
        let l = Luma {
            data: &buf,
            stride: 640,
            width: 640,
            height: 360,
            bytes: 1,
            depth: 8,
        };
        assert_eq!(l.barcode(), None);
    }

    #[test]
    fn crc_skips_stride_padding() {
        let c = Crc32::default();
        let a = [1u8, 2, 3, 9, 9, 4, 5, 6, 9, 9];
        let l = Luma {
            data: &a,
            stride: 5,
            width: 3,
            height: 2,
            bytes: 1,
            depth: 8,
        };
        assert_eq!(l.crc(&c), c.checksum(&[1, 2, 3, 4, 5, 6]));
    }
}
