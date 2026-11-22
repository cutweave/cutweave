//! MTSDF glyph atlas: on-demand generation (parallel across glyphs, deterministic results),
//! shelf packing into one RGBA8 texture that grows in height.
//!
//! Placement depends on the order glyphs are first requested, but rendering does not: the text
//! shader clamps every bilinear tap to the glyph's own cell, so a glyph renders identically
//! wherever it sits (cold vs warm frames stay bit-identical).

use std::collections::HashMap;
use std::time::Instant;

use super::fonts::FaceData;
use super::msdf::{GlyphField, MsdfConfig, generate};
use super::outline::glyph_outline;

/// Atlas width (texels); the height doubles as needed up to [`MAX_HEIGHT`].
pub const ATLAS_WIDTH: u32 = 1024;
pub const MAX_HEIGHT: u32 = 8192;
const PADDING: u32 = 1;

/// Process-unique revisions, so a replaced atlas never aliases the uploaded one.
fn next_revision() -> u64 {
    static REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    REV.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A glyph of a face at a variable-font weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphKey {
    pub face_hash: u64,
    pub glyph: u16,
    pub weight: u16,
}

/// Where a glyph lives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtlasEntry {
    /// Texel rect `[x, y, w, h]`; zero-sized for blank glyphs.
    pub rect: [u32; 4],
    /// Em-space bounds relative to the pen, y up: `[left, bottom, right, top]`.
    pub plane: [f32; 4],
}

impl AtlasEntry {
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.rect[2] == 0
    }
}

/// Generation statistics of one [`GlyphAtlas::ensure`] call.
#[derive(Clone, Copy, Debug, Default)]
pub struct EnsureStats {
    pub generated: usize,
    pub ms: f64,
    pub grew: bool,
}

/// The atlas (CPU side; the renderer uploads [`GlyphAtlas::pixels`] when [`GlyphAtlas::revision`]
/// changes).
#[derive(Debug)]
pub struct GlyphAtlas {
    pub config: MsdfConfig,
    pub width: u32,
    pub height: u32,
    pixels: Vec<u8>,
    entries: HashMap<GlyphKey, AtlasEntry>,
    shelves: Vec<(u32, u32, u32)>, // (y, height, next x)
    revision: u64,
    pub total_generated: usize,
    pub total_ms: f64,
}

impl GlyphAtlas {
    #[must_use]
    pub fn new(config: MsdfConfig) -> Self {
        let height = 256;
        Self {
            config,
            width: ATLAS_WIDTH,
            height,
            pixels: vec![0; (ATLAS_WIDTH * height * 4) as usize],
            entries: HashMap::new(),
            shelves: Vec::new(),
            revision: next_revision(),
            total_generated: 0,
            total_ms: 0.0,
        }
    }

    /// Increments whenever pixels change.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn get(&self, key: &GlyphKey) -> Option<&AtlasEntry> {
        self.entries.get(key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes of the CPU copy (the GPU texture is the same size).
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.pixels.len()
    }

    /// Generates and packs every missing glyph. Missing glyphs are generated in parallel and
    /// packed in sorted key order, so the atlas bytes depend only on the request sequence.
    pub fn ensure(&mut self, wanted: &[(GlyphKey, &FaceData)]) -> EnsureStats {
        let t0 = Instant::now();
        let mut missing: Vec<(GlyphKey, &FaceData)> = wanted
            .iter()
            .filter(|(k, _)| !self.entries.contains_key(k))
            .copied()
            .collect();
        missing.sort_by_key(|(k, _)| *k);
        missing.dedup_by_key(|(k, _)| *k);
        if missing.is_empty() {
            return EnsureStats::default();
        }
        let cfg = self.config;
        let threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZero::get)
            .min(missing.len());
        let chunk = missing.len().div_ceil(threads);
        let fields: Vec<Option<GlyphField>> = std::thread::scope(|s| {
            let handles: Vec<_> = missing
                .chunks(chunk)
                .map(|part| {
                    s.spawn(move || {
                        part.iter()
                            .map(|(k, face)| {
                                generate(&glyph_outline(face, k.glyph, k.weight), &cfg)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        });
        let mut grew = false;
        for ((key, _), field) in missing.iter().zip(fields) {
            let entry = match field {
                None => AtlasEntry {
                    rect: [0; 4],
                    plane: [0.0; 4],
                },
                Some(f) => {
                    let (x, y) = self.allocate(f.width, f.height, &mut grew);
                    self.blit(&f, x, y);
                    AtlasEntry {
                        rect: [x, y, f.width, f.height],
                        plane: f.plane,
                    }
                }
            };
            self.entries.insert(*key, entry);
        }
        self.revision = next_revision();
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        self.total_generated += missing.len();
        self.total_ms += ms;
        EnsureStats {
            generated: missing.len(),
            ms,
            grew,
        }
    }

    fn allocate(&mut self, w: u32, h: u32, grew: &mut bool) -> (u32, u32) {
        let (wp, hp) = (w + PADDING, h + PADDING);
        for shelf in &mut self.shelves {
            if hp <= shelf.1 && shelf.2 + wp <= self.width {
                let x = shelf.2;
                shelf.2 += wp;
                return (x, shelf.0);
            }
        }
        let y = self.shelves.last().map_or(0, |s| s.0 + s.1);
        while y + hp > self.height && self.height < MAX_HEIGHT {
            self.height *= 2;
            self.pixels
                .resize((self.width * self.height * 4) as usize, 0);
            *grew = true;
        }
        self.shelves.push((y, hp, wp));
        (0, y)
    }

    fn blit(&mut self, f: &GlyphField, x: u32, y: u32) {
        let row = (f.width * 4) as usize;
        for j in 0..f.height {
            if y + j >= self.height {
                break;
            }
            let dst = (((y + j) * self.width + x) * 4) as usize;
            let src = (j as usize) * row;
            self.pixels[dst..dst + row].copy_from_slice(&f.rgba[src..src + row]);
        }
    }
}
