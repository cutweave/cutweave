// SPDX-License-Identifier: GPL-3.0-or-later
//! The golden-frame harness: reference frames stored **per adapter class** and compared with
//! the §20 tolerances (arch §8, §20). Reusable by the WP-1O.01 `cutweave render` golden runner.
//!
//! Layout: `<root>/<adapter key>/<name>.png` (8-bit sRGB RGBA PNG with a `cICP` chunk), by
//! default `testdata/golden/render/` (Git LFS). The adapter key is
//! [`AdapterInfo::key`](crate::gpu::AdapterInfo::key): `dx12-warp` and `vulkan-llvmpipe` for the
//! CI software rasterisers, `<backend>-<gpu name>` for hardware.
//!
//! A frame is compared with its own adapter's reference when one exists, otherwise with a
//! fallback set (cross-adapter, normally the WARP set). Rule (single-sourced in
//! [`crate::metrics`]): pass when SSIM ≥ 0.995 and PSNR ≥ 40 dB; the report also says whether
//! the frame is bit-identical (expected on the same adapter and driver).
//!
//! `CUTWEAVE_UPDATE_GOLDENS=1` rewrites the adapter's own references instead of comparing.
//! Worktrees and LFS-less checkouts hold pointer files: those references are reported as
//! unavailable (`git lfs checkout testdata/golden` materialises them).

use std::fs::File;
use std::io::{BufReader, BufWriter, Read as _};
use std::path::{Path, PathBuf};

use crate::color::OutputTransfer;
use crate::image::Image;
use crate::metrics::{Comparison, compare};

/// Set to `1` to rewrite references instead of comparing.
pub const UPDATE_ENV: &str = "CUTWEAVE_UPDATE_GOLDENS";

/// H.273 `cICP` code points of the stored frames: BT.709 primaries, sRGB transfer, RGB, full
/// range.
const CICP_SRGB: [u8; 4] = [1, 13, 0, 1];

/// Golden I/O errors.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GoldenError {
    /// Reading or writing a file failed.
    #[error("{path}: {message}")]
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        message: String,
    },
}

fn io_err(path: &Path, e: impl std::fmt::Display) -> GoldenError {
    GoldenError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}

/// Writes an RGBA8 PNG (sRGB `cICP`).
pub fn write_png(path: &Path, img: &Image) -> Result<(), GoldenError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_err(path, e))?;
    }
    let file = File::create(path).map_err(|e| io_err(path, e))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| io_err(path, e))?;
    w.write_chunk(png::chunk::cICP, &CICP_SRGB)
        .map_err(|e| io_err(path, e))?;
    w.write_image_data(&img.data).map_err(|e| io_err(path, e))?;
    w.finish().map_err(|e| io_err(path, e))
}

/// Reads an 8-bit RGB or RGBA PNG as RGBA8.
pub fn read_png(path: &Path) -> Result<Image, GoldenError> {
    let file = File::open(path).map_err(|e| io_err(path, e))?;
    let mut reader = png::Decoder::new(BufReader::new(file))
        .read_info()
        .map_err(|e| io_err(path, e))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| io_err(path, "image too large"))?;
    let mut buf = vec![0u8; size];
    let info = reader.next_frame(&mut buf).map_err(|e| io_err(path, e))?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(io_err(
            path,
            format!("unsupported bit depth {:?}", info.bit_depth),
        ));
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
        other => return Err(io_err(path, format!("unsupported colour type {other:?}"))),
    };
    Ok(Image {
        width: info.width,
        height: info.height,
        data,
    })
}

/// Whether `path` is a Git LFS pointer rather than the content.
fn is_lfs_pointer(path: &Path) -> bool {
    let mut head = [0u8; 40];
    File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .is_ok_and(|n| head[..n].starts_with(b"version https://git-lfs"))
}

/// The outcome of one golden check.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Every byte matches the reference.
    BitIdentical,
    /// Within the §20 regression tolerance.
    WithinTolerance,
    /// Outside the tolerance.
    Failed,
    /// The reference was (re)written.
    Updated,
    /// No reference exists (neither the adapter's own nor the fallback).
    Missing,
    /// The reference is an un-materialised LFS pointer.
    Unavailable,
}

/// One golden comparison.
#[derive(Clone, Debug, PartialEq)]
pub struct GoldenReport {
    /// Frame name.
    pub name: String,
    /// The reference set compared against.
    pub reference_set: String,
    /// Whether that set belongs to the rendering adapter.
    pub same_adapter: bool,
    /// The metrics, when compared.
    pub comparison: Option<Comparison>,
    /// The outcome.
    pub verdict: Verdict,
}

impl GoldenReport {
    /// Whether the check passed (bit-identical, within tolerance or updated).
    #[must_use]
    pub fn passed(&self) -> bool {
        matches!(
            self.verdict,
            Verdict::BitIdentical | Verdict::WithinTolerance | Verdict::Updated
        )
    }
}

impl std::fmt::Display for GoldenReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:<28} vs {}{}: {:?}",
            self.name,
            self.reference_set,
            if self.same_adapter {
                ""
            } else {
                " (cross-adapter)"
            },
            self.verdict
        )?;
        if let Some(c) = &self.comparison {
            write!(f, " — {c}")?;
        }
        Ok(())
    }
}

/// A directory of per-adapter golden sets.
#[derive(Clone, Debug)]
pub struct GoldenStore {
    root: PathBuf,
}

impl GoldenStore {
    /// A store rooted at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> GoldenStore {
        GoldenStore { root: root.into() }
    }

    /// The compositor's references: `testdata/golden/render/` in the workspace.
    #[must_use]
    pub fn workspace() -> GoldenStore {
        GoldenStore::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("testdata")
                .join("golden")
                .join("render"),
        )
    }

    /// The root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file of frame `name` in set `set`.
    #[must_use]
    pub fn path(&self, set: &str, name: &str) -> PathBuf {
        self.root.join(set).join(format!("{name}.png"))
    }

    /// Adapter keys that have a set, sorted.
    #[must_use]
    pub fn sets(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.root).map_or_else(
            |_| Vec::new(),
            |rd| {
                rd.filter_map(Result::ok)
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            },
        );
        v.sort();
        v
    }

    /// Checks `image` (frame `name`, rendered on adapter `adapter`) against the adapter's own
    /// reference, else `fallback`'s; with `update`, writes the adapter's reference instead.
    pub fn check(
        &self,
        adapter: &str,
        fallback: &str,
        name: &str,
        image: &Image,
        update: bool,
    ) -> Result<GoldenReport, GoldenError> {
        let report = |set: &str, same: bool, comparison, verdict| GoldenReport {
            name: name.to_string(),
            reference_set: set.to_string(),
            same_adapter: same,
            comparison,
            verdict,
        };
        let own = self.path(adapter, name);
        if update {
            write_png(&own, image)?;
            return Ok(report(adapter, true, None, Verdict::Updated));
        }
        let (set, same) = if own.exists() {
            (adapter, true)
        } else {
            (fallback, adapter == fallback)
        };
        let path = self.path(set, name);
        if !path.exists() {
            return Ok(report(set, same, None, Verdict::Missing));
        }
        if is_lfs_pointer(&path) {
            return Ok(report(set, same, None, Verdict::Unavailable));
        }
        let reference = read_png(&path)?;
        let c = compare(image, &reference, OutputTransfer::Srgb);
        let verdict = if c.identical {
            Verdict::BitIdentical
        } else if c.passes_golden() {
            Verdict::WithinTolerance
        } else {
            Verdict::Failed
        };
        Ok(report(set, same, Some(c), verdict))
    }
}

/// Whether `CUTWEAVE_UPDATE_GOLDENS` asks for rewriting references.
#[must_use]
pub fn update_requested() -> bool {
    std::env::var(UPDATE_ENV).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::cast_possible_truncation)]

    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cutweave-golden-{tag}-{}", std::process::id()))
    }

    fn gradient(w: u32, h: u32, shift: u8) -> Image {
        let mut img = Image::filled(w, h, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 255) / w.max(1)) as u8;
                img.set_pixel(x, y, [v.saturating_add(shift), v, (y * 7) as u8, 255]);
            }
        }
        img
    }

    #[test]
    fn png_round_trip() {
        let root = temp_root("png");
        let img = gradient(13, 7, 0);
        let p = root.join("a.png");
        write_png(&p, &img).unwrap();
        assert_eq!(read_png(&p).unwrap(), img);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn own_set_fallback_update_and_lfs_pointers() {
        let root = temp_root("store");
        let store = GoldenStore::new(&root);
        let img = gradient(64, 32, 0);
        // Nothing stored yet.
        let r = store
            .check("dx12-gpu", "dx12-warp", "f", &img, false)
            .unwrap();
        assert_eq!(r.verdict, Verdict::Missing);
        // Write the WARP set; another adapter falls back to it.
        store
            .check("dx12-warp", "dx12-warp", "f", &img, true)
            .unwrap();
        let r = store
            .check("dx12-gpu", "dx12-warp", "f", &img, false)
            .unwrap();
        assert_eq!(
            (r.verdict.clone(), r.same_adapter),
            (Verdict::BitIdentical, false)
        );
        // A one-code difference passes the §20 tolerance; a large one fails.
        let r = store
            .check("dx12-warp", "dx12-warp", "f", &gradient(64, 32, 1), false)
            .unwrap();
        assert_eq!(r.verdict, Verdict::WithinTolerance);
        assert!(r.passed());
        let r = store
            .check("dx12-warp", "dx12-warp", "f", &gradient(64, 32, 60), false)
            .unwrap();
        assert_eq!(r.verdict, Verdict::Failed);
        assert!(!r.passed());
        // An LFS pointer is unavailable, not a failure of the frame.
        std::fs::write(
            store.path("dx12-warp", "g"),
            "version https://git-lfs.github.com/spec/v1\noid sha256:00\nsize 1\n",
        )
        .unwrap();
        let r = store
            .check("dx12-warp", "dx12-warp", "g", &img, false)
            .unwrap();
        assert_eq!(r.verdict, Verdict::Unavailable);
        assert_eq!(store.sets(), ["dx12-warp"]);
        let _ = std::fs::remove_dir_all(root);
    }
}
