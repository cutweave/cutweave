// SPDX-License-Identifier: GPL-3.0-or-later
//! The golden-frame comparator (WP-1O.01): one rendered frame against its reference **on the
//! same adapter** (cross-adapter comparison is never done, arch §20).
//!
//! The rule is single-sourced in `cutweave_render::metrics` (`Comparison::passes_golden`, the
//! phase tolerance table): a frame passes when it is bit-identical to its reference, or when
//! SSIM ≥ 0.995 **and** PSNR ≥ 40 dB. Spike S4 recommended PSNR and SSIM (plus max |Δ|, which
//! the report prints) and no perceptual metric, so there is no FLIP here.
//!
//! A failing frame is written three times for review: the actual frame, its reference and a
//! diff image (the reference dimmed to a quarter in grey, every differing pixel in red, brighter
//! for larger differences).

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use cutweave_render::Image;
use cutweave_render::color::OutputTransfer;
use cutweave_render::metrics::{self, Comparison};

/// The outcome of one comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every byte equals the reference (expected on the same adapter and driver).
    BitIdentical,
    /// Not identical, but within SSIM ≥ 0.995 and PSNR ≥ 40 dB.
    WithinTolerance,
    /// Outside the tolerance, or a different size.
    Failed,
}

impl Verdict {
    /// Whether the frame passes.
    pub fn passed(self) -> bool {
        self != Verdict::Failed
    }
}

/// A verdict with the metrics behind it (`None` when the images are identical or differ in
/// size, where no metric is needed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Judgement {
    /// The outcome.
    pub verdict: Verdict,
    /// The metrics.
    pub comparison: Option<Comparison>,
}

impl std::fmt::Display for Judgement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.verdict, &self.comparison) {
            (Verdict::BitIdentical, _) => f.write_str("bit-identical"),
            (Verdict::WithinTolerance, Some(c)) => write!(
                f,
                "within tolerance: PSNR {:.2} dB, SSIM {:.5}, max |Δ| {}",
                c.psnr_db, c.ssim, c.max_abs_diff
            ),
            (Verdict::Failed, Some(c)) => write!(
                f,
                "FAILED: PSNR {:.2} dB (min {}), SSIM {:.5} (min {}), max |Δ| {}",
                c.psnr_db,
                metrics::GOLDEN_MIN_PSNR_DB,
                c.ssim,
                metrics::GOLDEN_MIN_SSIM,
                c.max_abs_diff
            ),
            (Verdict::Failed, None) => f.write_str("FAILED: the image size differs"),
            (Verdict::WithinTolerance, None) => f.write_str("within tolerance"),
        }
    }
}

/// Judges `actual` against `expected` (both straight-alpha sRGB RGBA8).
pub fn judge(actual: &Image, expected: &Image) -> Judgement {
    if actual.width != expected.width || actual.height != expected.height {
        return Judgement {
            verdict: Verdict::Failed,
            comparison: None,
        };
    }
    if actual.data == expected.data {
        return Judgement {
            verdict: Verdict::BitIdentical,
            comparison: None,
        };
    }
    let c = metrics::compare(actual, expected, OutputTransfer::Srgb);
    Judgement {
        verdict: if c.passes_golden() {
            Verdict::WithinTolerance
        } else {
            Verdict::Failed
        },
        comparison: Some(c),
    }
}

/// The diff image: `expected` at a quarter of its luma in grey, every pixel whose colour or
/// alpha differs in red (`64 + 4·max|Δ|`, saturating). For different sizes, the overlapping
/// area is compared and the rest is marked as different.
pub fn diff_image(actual: &Image, expected: &Image) -> Image {
    let width = actual.width.max(expected.width);
    let height = actual.height.max(expected.height);
    let mut out = Image::filled(width, height, [255, 0, 0, 255]);
    let pixel = |img: &Image, x: u32, y: u32| -> Option<[u8; 4]> {
        (x < img.width && y < img.height).then(|| img.pixel(x, y))
    };
    for y in 0..height {
        for x in 0..width {
            let (Some(got), Some(want)) = (pixel(actual, x, y), pixel(expected, x, y)) else {
                continue;
            };
            let delta = got
                .iter()
                .zip(want)
                .map(|(p, q)| p.abs_diff(q))
                .max()
                .unwrap_or(0);
            let value = if delta == 0 {
                let luma = (2126 * u32::from(want[0])
                    + 7152 * u32::from(want[1])
                    + 722 * u32::from(want[2]))
                    / 10_000;
                let grey = u8::try_from(luma / 4).unwrap_or(u8::MAX);
                [grey, grey, grey, 255]
            } else {
                [64u8.saturating_add(delta.saturating_mul(4)), 0, 0, 255]
            };
            out.set_pixel(x, y, value);
        }
    }
    out
}

/// Reads an 8-bit RGB or RGBA PNG as RGBA8.
pub fn read_png(path: &Path) -> Result<Image, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let file = File::open(path).map_err(|e| err(&e))?;
    let mut reader = png::Decoder::new(BufReader::new(file))
        .read_info()
        .map_err(|e| err(&e))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| err(&"image too large"))?;
    let mut buf = vec![0u8; size];
    let info = reader.next_frame(&mut buf).map_err(|e| err(&e))?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(err(&format!("unsupported bit depth {:?}", info.bit_depth)));
    }
    let bytes = &buf[..info.buffer_size()];
    let data = match info.color_type {
        png::ColorType::Rgba => bytes.to_vec(),
        png::ColorType::Rgb => bytes
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        other => return Err(err(&format!("unsupported colour type {other:?}"))),
    };
    Ok(Image {
        width: info.width,
        height: info.height,
        data,
    })
}

/// H.273 `cICP` code points of the frames: BT.709 primaries, sRGB transfer, RGB, full range
/// (what `render.frame` writes).
const CICP_SRGB: [u8; 4] = [1, 13, 0, 1];

/// Writes `img` as a PNG: RGB when every pixel is opaque, else RGBA, with a `cICP` chunk.
/// `small` spends more time for a smaller file (stored references).
pub fn write_png(path: &Path, img: &Image, small: bool) -> Result<(), String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| err(&e))?;
    }
    let pixels = img.data.as_chunks::<4>().0;
    let opaque = pixels.iter().all(|p| p[3] == 255);
    let file = File::create(path).map_err(|e| err(&e))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), img.width, img.height);
    let data: Vec<u8> = if opaque {
        enc.set_color(png::ColorType::Rgb);
        pixels.iter().flat_map(|p| [p[0], p[1], p[2]]).collect()
    } else {
        enc.set_color(png::ColorType::Rgba);
        img.data.clone()
    };
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(if small {
        png::Compression::High
    } else {
        png::Compression::Fast
    });
    let mut w = enc.write_header().map_err(|e| err(&e))?;
    w.write_chunk(png::chunk::cICP, &CICP_SRGB)
        .map_err(|e| err(&e))?;
    w.write_image_data(&data).map_err(|e| err(&e))?;
    w.finish().map_err(|e| err(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A colour-bars-like test card: eight vertical bars over a horizontal ramp.
    fn card(w: u32, h: u32, shift: u32) -> Image {
        const BARS: [[u8; 3]; 8] = [
            [235, 235, 235],
            [235, 235, 16],
            [16, 235, 235],
            [16, 235, 16],
            [235, 16, 235],
            [235, 16, 16],
            [16, 16, 235],
            [16, 16, 16],
        ];
        let mut img = Image::filled(w, h, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 0..w {
                let sx = x.saturating_sub(shift);
                let rgb = if y < h * 2 / 3 {
                    BARS[(sx * 8 / w) as usize]
                } else {
                    let v = u8::try_from(sx * 255 / w).unwrap_or(255);
                    [v, v, v]
                };
                img.set_pixel(x, y, [rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        img
    }

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("xtask-golden-{name}-{}", std::process::id()))
    }

    #[test]
    fn identical_frames_are_bit_identical() {
        let a = card(64, 36, 0);
        let j = judge(&a, &a.clone());
        assert_eq!(j.verdict, Verdict::BitIdentical);
        assert!(j.verdict.passed() && j.comparison.is_none());
    }

    #[test]
    fn off_by_one_everywhere_is_within_tolerance() {
        // Every R, G and B sample one code up: PSNR = 20·log10(255) ≈ 48.1 dB.
        let a = card(64, 36, 0);
        let mut b = a.clone();
        for p in b.data.as_chunks_mut::<4>().0 {
            for c in &mut p[..3] {
                *c = c.saturating_add(1);
            }
        }
        let j = judge(&b, &a);
        assert_eq!(j.verdict, Verdict::WithinTolerance, "{j}");
        let c = j.comparison.expect("metrics");
        assert!((c.psnr_db - 48.13).abs() < 0.1 && c.ssim >= metrics::GOLDEN_MIN_SSIM);
    }

    #[test]
    fn a_one_pixel_shift_fails() {
        let a = card(640, 360, 0);
        let b = card(640, 360, 1);
        let j = judge(&b, &a);
        assert_eq!(j.verdict, Verdict::Failed, "{j}");
        let c = j.comparison.expect("metrics");
        assert!(c.psnr_db < metrics::GOLDEN_MIN_PSNR_DB, "{j}");
    }

    #[test]
    fn a_different_size_fails() {
        let j = judge(&card(64, 36, 0), &card(64, 37, 0));
        assert_eq!(j.verdict, Verdict::Failed);
        assert_eq!(j.to_string(), "FAILED: the image size differs");
    }

    #[test]
    fn the_diff_marks_exactly_the_differing_pixels() {
        let a = card(64, 36, 0);
        let mut b = a.clone();
        b.set_pixel(10, 5, [0, 0, 0, 255]);
        let d = diff_image(&b, &a);
        let at = |x: u32, y: u32| {
            let i = ((y * d.width + x) * 4) as usize;
            [d.data[i], d.data[i + 1], d.data[i + 2]]
        };
        assert_eq!(at(10, 5), [255, 0, 0]);
        let other = at(11, 5);
        assert_eq!(other[0], other[1]);
        assert!(other[0] < 64);
    }

    #[test]
    fn png_round_trip_keeps_pixels_and_alpha() {
        let dir = temp("png");
        let opaque = card(33, 17, 0);
        let mut translucent = opaque.clone();
        translucent.set_pixel(3, 3, [1, 2, 3, 4]);
        for (name, img) in [("o.png", &opaque), ("t.png", &translucent)] {
            for small in [false, true] {
                let p = dir.join(format!("{small}-{name}"));
                write_png(&p, img, small).unwrap();
                assert_eq!(&read_png(&p).unwrap(), img);
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
