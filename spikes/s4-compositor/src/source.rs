//! Source frames (what a decoder hands the renderer) and the provider abstraction.
//!
//! In the product, `cutweave-media` decodes NV12/P010 planes and probes colour tags (§7); here
//! [`SyntheticSources`] generates equivalent planes in code (no FFmpeg on the build machine).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::hash::Fnv;
use crate::lut::{BUNDLED_TEST_LUT, Lut3d};
use crate::patterns;
use crate::scene::ColorTags;
use crate::{Error, Result};

/// Raw pixel payload of a decoded frame.
#[derive(Clone, Debug, PartialEq)]
pub enum PixelData {
    /// Planar Y'CbCr 4:2:0 code values (u16 per sample; low `bit_depth` bits used). Chroma
    /// planes are `ceil(w/2) × ceil(h/2)` with MPEG-2 ("left") siting: co-sited horizontally,
    /// interstitial vertically.
    Yuv420 {
        bit_depth: u8,
        y: Vec<u16>,
        cb: Vec<u16>,
        cr: Vec<u16>,
    },
    /// Interleaved 8-bit R'G'B'A, straight (non-premultiplied) alpha.
    Rgba8(Vec<u8>),
}

/// A decoded frame plus its probe tags.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: PixelData,
    pub tags: ColorTags,
    /// Stable identity of the pixel content (media hash + pts in the product).
    pub content_hash: u64,
}

impl SourceFrame {
    /// Chroma plane dimensions for 4:2:0.
    #[must_use]
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }
}

/// Resolves media and LUT references in a scene.
pub trait SourceProvider: Send + Sync {
    /// The frame `frame` of media `id`.
    fn frame(&self, id: &str, frame: i64) -> Result<Arc<SourceFrame>>;
    /// The LUT `id`.
    fn lut(&self, id: &str) -> Result<Arc<Lut3d>>;
}

/// Synthetic test media generated in code, sized relative to a canvas.
///
/// | id          | format                                   | content                                  |
/// |-------------|------------------------------------------|------------------------------------------|
/// | `camera709` | 8-bit 4:2:0, BT.709 matrix, limited, BT.709 TF | 75% bars, ramp, grey steps, moving bar (time-varying) |
/// | `hdr10`     | 10-bit 4:2:0, BT.2020 NCL, limited, PQ    | 0–4000 cd/m² ramp, BT.2020 primaries at 1000 cd/m², grey steps |
/// | `phone601`  | 8-bit 4:2:0, BT.601 matrix, full range, sRGB TF | hue/value sweep with zone-plate rings |
/// | `chart`     | RGBA8 sRGB, straight alpha               | 24-patch colour checker on a rounded card |
pub struct SyntheticSources {
    canvas: (u32, u32),
    cache: Mutex<HashMap<String, Arc<SourceFrame>>>,
    luts: HashMap<String, Arc<Lut3d>>,
}

impl SyntheticSources {
    /// All source ids.
    pub const IDS: [&'static str; 4] = ["camera709", "hdr10", "phone601", "chart"];

    /// Sources scaled for a `canvas_w × canvas_h` sequence.
    pub fn new(canvas_w: u32, canvas_h: u32) -> Result<Self> {
        let mut luts = HashMap::new();
        luts.insert(
            "warm_contrast".to_string(),
            Arc::new(Lut3d::parse_cube(BUNDLED_TEST_LUT)?),
        );
        luts.insert("identity".to_string(), Arc::new(Lut3d::identity(2)));
        Ok(Self {
            canvas: (canvas_w, canvas_h),
            cache: Mutex::new(HashMap::new()),
            luts,
        })
    }

    /// Native size of a source for this canvas.
    #[must_use]
    pub fn source_size(&self, id: &str) -> Option<(u32, u32)> {
        let (w, h) = self.canvas;
        let even = |v: f64| ((v / 2.0).round() as u32).max(1) * 2;
        match id {
            "camera709" => Some((w, h)),
            "hdr10" | "phone601" => Some((even(f64::from(w) / 2.0), even(f64::from(h) / 2.0))),
            "chart" => Some((even(f64::from(w) * 0.36), even(f64::from(w) * 0.24))),
            _ => None,
        }
    }

    fn generate(&self, id: &str, frame: i64) -> Result<SourceFrame> {
        let (w, h) = self
            .source_size(id)
            .ok_or_else(|| Error::UnknownSource(id.into()))?;
        let mut hash = Fnv::new();
        hash.str("synthetic").str(id).u32(w).u32(h);
        let (pixels, tags) = match id {
            "camera709" => {
                hash.i64(frame);
                patterns::camera709(w, h, frame)
            }
            "hdr10" => patterns::hdr10(w, h),
            "phone601" => patterns::phone601(w, h),
            "chart" => patterns::chart(w, h),
            _ => return Err(Error::UnknownSource(id.into())),
        };
        Ok(SourceFrame {
            width: w,
            height: h,
            pixels,
            tags,
            content_hash: hash.finish(),
        })
    }
}

impl SourceProvider for SyntheticSources {
    fn frame(&self, id: &str, frame: i64) -> Result<Arc<SourceFrame>> {
        // Static sources are cached; the time-varying camera is regenerated per frame (a decoder
        // would hand over a new frame anyway).
        let is_static = id != "camera709";
        if is_static {
            let cache = self
                .cache
                .lock()
                .map_err(|_| Error::Gpu("source cache poisoned".into()))?;
            if let Some(f) = cache.get(id) {
                return Ok(Arc::clone(f));
            }
        }
        let f = Arc::new(self.generate(id, frame)?);
        if is_static {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| Error::Gpu("source cache poisoned".into()))?;
            cache.insert(id.to_string(), Arc::clone(&f));
        }
        Ok(f)
    }

    fn lut(&self, id: &str) -> Result<Arc<Lut3d>> {
        self.luts
            .get(id)
            .cloned()
            .ok_or_else(|| Error::UnknownLut(id.into()))
    }
}
