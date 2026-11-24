// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical Y'CbCr planes for comparing hardware and software decodes (WP-0E.02).
//!
//! Software decode returns planar `yuv420p` / `yuv420p10le` (10-bit samples in the low bits);
//! `av_hwframe_transfer_data` returns semi-planar NV12 / P010 (10-bit samples in the **high**
//! bits, interleaved CbCr). Both are normalised to three planar planes of LSB-aligned samples, so
//! a bit-exact decode gives identical plane CRCs and anything else gets a PSNR.

use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;

use crate::util::Crc32;

/// Planar 4:2:0 samples, LSB-aligned, no stride padding.
#[derive(Clone, Debug, PartialEq)]
pub struct Planes {
    pub width: usize,
    pub height: usize,
    pub depth: u32,
    pub y: Vec<u16>,
    pub cb: Vec<u16>,
    pub cr: Vec<u16>,
}

/// PSNR of one comparison, in dB (`f64::INFINITY` when identical).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Psnr {
    pub y: f64,
    pub cb: f64,
    pub cr: f64,
    /// Over all samples of the three planes.
    pub all: f64,
    /// Largest absolute sample difference.
    pub max_diff: u32,
}

/// The pixel layouts the comparison understands: `(semi-planar, bytes per sample, LSB shift)`.
fn layout(f: Pixel) -> Option<(bool, usize, u32, u32)> {
    // (semi-planar, bytes per sample, right shift to LSB-align, bit depth)
    Some(match f {
        Pixel::YUV420P | Pixel::YUVJ420P => (false, 1, 0, 8),
        Pixel::YUV420P10LE => (false, 2, 0, 10),
        Pixel::NV12 => (true, 1, 0, 8),
        Pixel::P010LE => (true, 2, 6, 10),
        Pixel::P016LE => (true, 2, 0, 16),
        _ => return None,
    })
}

fn sample(row: &[u8], x: usize, bytes: usize, shift: u32) -> u16 {
    if bytes == 1 {
        u16::from(row[x])
    } else {
        u16::from_le_bytes([row[2 * x], row[2 * x + 1]]) >> shift
    }
}

impl Planes {
    /// Extracts the planes of a decoded 4:2:0 frame; `None` for other formats.
    pub fn of(frame: &ffmpeg::frame::Video) -> Option<Self> {
        let (semi, bytes, shift, depth) = layout(frame.format())?;
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let need = if semi { 2 } else { 3 };
        if frame.planes() < need {
            return None;
        }
        let mut y = Vec::with_capacity(w * h);
        let (d0, s0) = (frame.data(0), frame.stride(0));
        for r in 0..h {
            let row = &d0[r * s0..];
            y.extend((0..w).map(|x| sample(row, x, bytes, shift)));
        }
        let mut cb = Vec::with_capacity(cw * ch);
        let mut cr = Vec::with_capacity(cw * ch);
        if semi {
            let (d1, s1) = (frame.data(1), frame.stride(1));
            for r in 0..ch {
                let row = &d1[r * s1..];
                for x in 0..cw {
                    cb.push(sample(row, 2 * x, bytes, shift));
                    cr.push(sample(row, 2 * x + 1, bytes, shift));
                }
            }
        } else {
            let (d1, s1, d2, s2) = (
                frame.data(1),
                frame.stride(1),
                frame.data(2),
                frame.stride(2),
            );
            for r in 0..ch {
                let (a, b) = (&d1[r * s1..], &d2[r * s2..]);
                cb.extend((0..cw).map(|x| sample(a, x, bytes, shift)));
                cr.extend((0..cw).map(|x| sample(b, x, bytes, shift)));
            }
        }
        Some(Planes {
            width: w,
            height: h,
            depth,
            y,
            cb,
            cr,
        })
    }

    /// CRC-32 of each plane over the little-endian u16 samples.
    pub fn crcs(&self, crc: &Crc32) -> [u32; 3] {
        let h = |p: &[u16]| {
            let mut s = 0xFFFF_FFFF;
            for chunk in p.chunks(4096) {
                let bytes: Vec<u8> = chunk.iter().flat_map(|v| v.to_le_bytes()).collect();
                s = crc.update(s, &bytes);
            }
            s ^ 0xFFFF_FFFF
        };
        [h(&self.y), h(&self.cb), h(&self.cr)]
    }

    /// PSNR against `reference` (same size and depth), peak = 2^depth - 1.
    pub fn psnr(&self, reference: &Planes) -> Option<Psnr> {
        if (self.width, self.height, self.depth)
            != (reference.width, reference.height, reference.depth)
            || self.y.len() != reference.y.len()
            || self.cb.len() != reference.cb.len()
        {
            return None;
        }
        let peak = f64::from((1u32 << self.depth) - 1);
        let mut max_diff = 0u32;
        let mut sse = |a: &[u16], b: &[u16]| {
            let mut s = 0u64;
            for (x, y) in a.iter().zip(b) {
                let d = u32::from(x.abs_diff(*y));
                max_diff = max_diff.max(d);
                s += u64::from(d * d);
            }
            s
        };
        let (sy, sb, sr) = (
            sse(&self.y, &reference.y),
            sse(&self.cb, &reference.cb),
            sse(&self.cr, &reference.cr),
        );
        let db = |s: u64, n: usize| {
            if s == 0 {
                f64::INFINITY
            } else {
                10.0 * (peak * peak / (s as f64 / n as f64)).log10()
            }
        };
        let (ny, nc) = (self.y.len(), self.cb.len());
        Some(Psnr {
            y: db(sy, ny),
            cb: db(sb, nc),
            cr: db(sr, nc),
            all: db(sy + sb + sr, ny + 2 * nc),
            max_diff,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planes(depth: u32, f: impl Fn(usize) -> u16) -> Planes {
        let (w, h) = (8, 4);
        Planes {
            width: w,
            height: h,
            depth,
            y: (0..w * h).map(&f).collect(),
            cb: (0..w * h / 4).map(&f).collect(),
            cr: (0..w * h / 4).map(&f).collect(),
        }
    }

    #[test]
    fn identical_planes_are_infinite_psnr() {
        let a = planes(10, |i| (i * 37 % 1024) as u16);
        let p = a.psnr(&a).expect("same size");
        assert!(p.all.is_infinite() && p.y.is_infinite());
        assert_eq!(p.max_diff, 0);
        let c = Crc32::default();
        assert_eq!(a.crcs(&c), a.clone().crcs(&c));
    }

    #[test]
    fn psnr_of_a_known_error() {
        // Every sample off by one at 8 bits: MSE 1 -> 20 log10(255) = 48.13 dB.
        let a = planes(8, |i| (i % 200) as u16 + 10);
        let b = planes(8, |i| (i % 200) as u16 + 11);
        let p = a.psnr(&b).expect("same size");
        assert!((p.all - 48.13).abs() < 0.01, "{p:?}");
        assert_eq!(p.max_diff, 1);
        assert_ne!(a.crcs(&Crc32::default()), b.crcs(&Crc32::default()));
    }

    #[test]
    fn p010_samples_are_shifted_down() {
        // 0x3FF << 6 = 0xFFC0 in P010 reads back as 1023.
        let row = 0xFFC0u16.to_le_bytes();
        assert_eq!(sample(&row, 0, 2, 6), 1023);
        assert_eq!(sample(&[200u8], 0, 1, 0), 200);
    }

    #[test]
    fn size_mismatch_has_no_psnr() {
        let a = planes(8, |_| 1);
        let mut b = a.clone();
        b.depth = 10;
        assert!(a.psnr(&b).is_none());
    }
}
