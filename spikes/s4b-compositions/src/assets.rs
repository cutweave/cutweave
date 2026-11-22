// SPDX-License-Identifier: GPL-3.0-or-later
//! Asset resolution for `<Image src="...">`.
//!
//! Compositions cannot read files: `src` is an **asset id** the host resolves (project media bin
//! in the product). Here: S4's synthetic sources (`camera709`, `hdr10`, `phone601`, `chart`) plus
//! a procedurally generated `logo` (RGBA8, straight alpha).

use std::collections::HashMap;
use std::sync::Arc;

use s4::hash::Fnv;
use s4::lut::Lut3d;
use s4::scene::{ColorTags, Matrix, Primaries, Range, Transfer};
use s4::source::{PixelData, SourceFrame, SourceProvider, SyntheticSources};

/// Synthetic media + generated image assets.
pub struct AssetSources {
    synthetic: SyntheticSources,
    assets: HashMap<String, Arc<SourceFrame>>,
}

impl AssetSources {
    /// Sources sized for a `width × height` canvas.
    pub fn new(width: u32, height: u32) -> s4::Result<Self> {
        let mut assets = HashMap::new();
        assets.insert("logo".to_string(), Arc::new(logo(256)));
        Ok(Self {
            synthetic: SyntheticSources::new(width, height)?,
            assets,
        })
    }

    /// Registered asset ids.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        let mut v: Vec<String> = SyntheticSources::IDS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        v.extend(self.assets.keys().cloned());
        v.sort();
        v
    }
}

impl SourceProvider for AssetSources {
    fn frame(&self, id: &str, frame: i64) -> s4::Result<Arc<SourceFrame>> {
        match self.assets.get(id) {
            Some(a) => Ok(Arc::clone(a)),
            None => self.synthetic.frame(id, frame),
        }
    }

    fn lut(&self, id: &str) -> s4::Result<Arc<Lut3d>> {
        self.synthetic.lut(id)
    }
}

/// A round badge: violet→teal gradient disc with a white play triangle (sRGB, straight alpha).
fn logo(size: u32) -> SourceFrame {
    let n = f64::from(size);
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    let c = n / 2.0;
    let r = n / 2.0 - 2.0;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let d = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt();
            let a = (r - d + 0.5).clamp(0.0, 1.0);
            let t = (fx + fy) / (2.0 * n);
            let mut rgb = [
                0.486 * (1.0 - t) + 0.078 * t,
                0.361 * (1.0 - t) + 0.722 * t,
                1.0 * (1.0 - t) + 0.651 * t,
            ];
            // Play triangle: (0.40, 0.30) – (0.40, 0.70) – (0.72, 0.50) in unit coordinates.
            let (u, v) = (fx / n, fy / n);
            let inside = u >= 0.40 && (v - 0.5).abs() <= 0.20 * (0.72 - u) / 0.32;
            if inside {
                rgb = [1.0, 1.0, 1.0];
            }
            for ch in rgb {
                px.push((ch * 255.0).round() as u8);
            }
            px.push((a * 255.0).round() as u8);
        }
    }
    let mut h = Fnv::new();
    h.str("s4b-logo").u32(size);
    SourceFrame {
        width: size,
        height: size,
        pixels: PixelData::Rgba8(px),
        tags: ColorTags {
            matrix: Matrix::Rgb,
            range: Range::Full,
            transfer: Transfer::Srgb,
            primaries: Primaries::Bt709,
        },
        content_hash: h.finish(),
    }
}
