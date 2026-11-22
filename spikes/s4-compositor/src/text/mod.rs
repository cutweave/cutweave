//! Text for the S4 spike (WP-0F.02): bundled fonts → cosmic-text shaping → resolved glyph list
//! (per-frame span animation applied) → either the MTSDF atlas pass or Vello.
//!
//! ```text
//! TextNode ──layout (cached)──► TextLayout ──resolve(spans)──► ResolvedText ─┬─► msdf instances (gpu)
//!                                                                             ├─► vello::Scene
//!                                                                             └─► raster (CPU reference)
//! ```

pub mod atlas;
pub mod caption;
pub mod fonts;
pub mod layout;
pub mod msdf;
pub mod outline;
pub mod raster;
pub mod vello_scene;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use cosmic_text::fontdb;

use crate::Result;
use crate::color::srgb_fill_to_linear_premul;
use crate::scene::{Affine, TextNode};
use atlas::{GlyphAtlas, GlyphKey};
use fonts::FontLibrary;
use layout::{TextLayout, layout, layout_key};
use msdf::MsdfConfig;

/// Karaoke sweep of one glyph (layout space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Highlight {
    /// Linear premultiplied colour.
    pub color: [f32; 4],
    /// Layout-space x of the sweep front.
    pub sweep_x: f32,
    /// +1: highlighted left of the front (LTR); −1: right of it (RTL).
    pub dir: f32,
}

/// One glyph ready to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedGlyph {
    pub face: fontdb::ID,
    pub key: GlyphKey,
    /// Baseline pen position (layout px = box px before animation).
    pub pen: [f32; 2],
    pub size: f32,
    pub advance: f32,
    /// Animation transform: layout px → box px.
    pub anim: [f32; 6],
    pub opacity: f32,
    /// Linear premultiplied fill.
    pub fill: [f32; 4],
    pub highlight: Option<Highlight>,
}

/// Drop shadow in linear premultiplied colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedShadow {
    pub offset: [f32; 2],
    pub blur: f32,
    pub color: [f32; 4],
}

/// A text node resolved for one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedText {
    pub glyphs: Vec<ResolvedGlyph>,
    /// Width (layout px) and linear premultiplied colour.
    pub outline: Option<(f32, [f32; 4])>,
    pub shadow: Option<ResolvedShadow>,
    pub box_size: [f32; 2],
}

/// `T(pivot + offset) · R(rot) · S(scale) · T(−pivot)`, clockwise degrees (y down), like
/// [`crate::scene::Transform`].
#[must_use]
pub fn about(pivot: [f32; 2], offset: [f32; 2], scale: [f32; 2], rotation_deg: f32) -> [f32; 6] {
    let (s, c) = rotation_deg.to_radians().sin_cos();
    let (a, b, d, e) = (c * scale[0], -s * scale[1], s * scale[0], c * scale[1]);
    let tx = pivot[0] + offset[0] - (a * pivot[0] + b * pivot[1]);
    let ty = pivot[1] + offset[1] - (d * pivot[0] + e * pivot[1]);
    [a, b, tx, d, e, ty]
}

/// Identity affine.
pub const IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

/// `outer ∘ inner` for f32 affines.
#[must_use]
pub fn compose(outer: &[f32; 6], inner: &[f32; 6]) -> [f32; 6] {
    let [a, b, c, d, e, f] = *outer;
    let [a2, b2, c2, d2, e2, f2] = *inner;
    [
        a * a2 + b * d2,
        a * b2 + b * e2,
        a * c2 + b * f2 + c,
        d * a2 + e * d2,
        d * b2 + e * e2,
        d * c2 + e * f2 + f,
    ]
}

/// f64 → f32 affine.
#[must_use]
pub fn affine_f32(m: &Affine) -> [f32; 6] {
    m.map(|v| v as f32)
}

/// Applies the node's spans to a layout.
#[must_use]
pub fn resolve(node: &TextNode, layout: &TextLayout) -> ResolvedText {
    let fill = srgb_fill_to_linear_premul(node.fill_srgb);
    let spans = &node.spans;
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans[i].range[0]);
    let span_of = |byte: u32| -> Option<usize> {
        let n = order.partition_point(|&i| spans[i].range[0] <= byte);
        let i = *order.get(n.checked_sub(1)?)?;
        (byte < spans[i].range[1]).then_some(i)
    };
    // Span boxes (layout space) and direction.
    let mut geo = vec![[f32::MAX, f32::MAX, f32::MIN, f32::MIN]; spans.len()];
    let mut rtl = vec![0i32; spans.len()];
    let mut which = Vec::with_capacity(layout.glyphs.len());
    for g in &layout.glyphs {
        let s = span_of(g.range[0]);
        if let Some(si) = s {
            let line = layout.lines[g.line as usize];
            let b = &mut geo[si];
            b[0] = b[0].min(g.pen[0]);
            b[1] = b[1].min(line.top);
            b[2] = b[2].max(g.pen[0] + g.advance);
            b[3] = b[3].max(line.top + line.height);
            rtl[si] += if g.rtl { 1 } else { -1 };
        }
        which.push(s);
    }
    let mut glyphs = Vec::with_capacity(layout.glyphs.len());
    for (g, s) in layout.glyphs.iter().zip(which) {
        let mut r = ResolvedGlyph {
            face: g.face,
            key: g.key,
            pen: g.pen,
            size: g.size,
            advance: g.advance,
            anim: IDENTITY,
            opacity: 1.0,
            fill,
            highlight: None,
        };
        if let Some(si) = s {
            let span = &spans[si];
            let b = geo[si];
            let pivot = if span.per_glyph {
                [g.pen[0] + g.advance * 0.5, g.pen[1] - 0.35 * g.size]
            } else {
                [(b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5]
            };
            r.anim = about(pivot, span.offset_px, span.scale, span.rotation_deg);
            r.opacity = if span.opacity.is_nan() {
                0.0
            } else {
                span.opacity.clamp(0.0, 1.0)
            };
            if let Some(f) = span.fill_srgb {
                r.fill = srgb_fill_to_linear_premul(f);
            }
            if let Some(h) = span.highlight {
                let p = h.progress.clamp(0.0, 1.0);
                let dir = if rtl[si] > 0 { -1.0 } else { 1.0 };
                let sweep_x = if dir > 0.0 {
                    b[0] + p * (b[2] - b[0])
                } else {
                    b[2] - p * (b[2] - b[0])
                };
                r.highlight = Some(Highlight {
                    color: srgb_fill_to_linear_premul(h.color_srgb),
                    sweep_x,
                    dir,
                });
            }
        }
        if r.opacity > 0.0 {
            glyphs.push(r);
        }
    }
    ResolvedText {
        glyphs,
        outline: node
            .outline
            .filter(|o| o.width_px > 0.0)
            .map(|o| (o.width_px, srgb_fill_to_linear_premul(o.color_srgb))),
        shadow: node.shadow.map(|s| ResolvedShadow {
            offset: s.offset_px,
            blur: s.blur_px.max(0.0),
            color: srgb_fill_to_linear_premul(s.color_srgb),
        }),
        box_size: node.box_size,
    }
}

/// CPU timing of the last frame's text work.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextCpuStats {
    /// Shaping + layout (0 on a layout-cache hit).
    pub layout_ms: f64,
    /// Span resolution + instance (or Vello scene) build.
    pub build_ms: f64,
    /// MTSDF generation of glyphs new to the atlas.
    pub atlas_ms: f64,
    pub glyphs: usize,
    pub instances: usize,
}

/// CPU side of the text renderer: fonts, layout cache, atlas.
#[derive(Debug)]
pub struct TextEngine {
    pub fonts: FontLibrary,
    pub atlas: GlyphAtlas,
    layouts: HashMap<u64, Arc<TextLayout>>,
}

/// Layout cache bound (entries); cleared wholesale when exceeded (spike policy).
const LAYOUT_CACHE: usize = 256;

impl TextEngine {
    pub fn new() -> Result<Self> {
        Ok(Self {
            fonts: FontLibrary::bundled()?,
            atlas: GlyphAtlas::new(MsdfConfig::default()),
            layouts: HashMap::new(),
        })
    }

    /// Cached layout of `node`; the bool is true when it was shaped now.
    pub fn layout(&mut self, node: &TextNode) -> (Arc<TextLayout>, bool) {
        let k = layout_key(node);
        if let Some(l) = self.layouts.get(&k) {
            return (Arc::clone(l), false);
        }
        if self.layouts.len() >= LAYOUT_CACHE {
            self.layouts.clear();
        }
        let l = Arc::new(layout(&mut self.fonts, node));
        self.layouts.insert(k, Arc::clone(&l));
        (l, true)
    }

    /// Layout + resolve, with timings.
    pub fn prepare(&mut self, node: &TextNode) -> (ResolvedText, TextCpuStats) {
        let t0 = Instant::now();
        let (l, _) = self.layout(node);
        let t1 = Instant::now();
        let r = resolve(node, &l);
        let stats = TextCpuStats {
            layout_ms: (t1 - t0).as_secs_f64() * 1e3,
            build_ms: t1.elapsed().as_secs_f64() * 1e3,
            glyphs: r.glyphs.len(),
            ..TextCpuStats::default()
        };
        (r, stats)
    }

    /// Makes sure every glyph of `r` is in the atlas.
    pub fn ensure_glyphs(&mut self, r: &ResolvedText) -> atlas::EnsureStats {
        let mut wanted = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for g in &r.glyphs {
            if self.atlas.get(&g.key).is_none()
                && seen.insert(g.key)
                && let Some(face) = self.fonts.face(g.face)
            {
                wanted.push((g.key, face));
            }
        }
        if wanted.is_empty() {
            return atlas::EnsureStats::default();
        }
        // Disjoint borrows: faces from `self.fonts`, mutation of `self.atlas`.
        self.atlas.ensure(&wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{TextHighlight, TextSpan};

    #[test]
    fn spans_animate_words_and_sweep_follows_reading_direction() {
        let mut eng = TextEngine::new().expect("engine");
        let mut node = TextNode::new(
            "hello مرحبا",
            "Bricolage Grotesque 800",
            64.0,
            [1.0; 4],
            [1200.0, 200.0],
        );
        let mut pop = TextSpan::new([0, 5]);
        pop.scale = [1.5, 1.5];
        pop.highlight = Some(TextHighlight {
            color_srgb: [1.0, 0.8, 0.0, 1.0],
            progress: 0.25,
        });
        let ar = "hello ".len() as u32;
        let mut arabic = TextSpan::new([ar, node.text.len() as u32]);
        arabic.highlight = Some(TextHighlight {
            color_srgb: [1.0, 0.8, 0.0, 1.0],
            progress: 0.25,
        });
        node.spans = vec![arabic, pop];
        let (r, _) = eng.prepare(&node);
        let first = r.glyphs.iter().find(|g| g.highlight.is_some()).expect("hl");
        assert_eq!(first.anim[0], 1.5);
        assert_eq!(first.highlight.expect("hl").dir, 1.0);
        let rtl = r
            .glyphs
            .iter()
            .find(|g| g.highlight.is_some_and(|h| h.dir < 0.0))
            .expect("rtl sweep");
        assert_eq!(rtl.anim, IDENTITY);
        // Unstyled space between the spans keeps the identity transform and the node fill.
        assert!(r.glyphs.iter().any(|g| g.highlight.is_none()));
    }

    #[test]
    fn about_keeps_the_pivot_fixed() {
        let m = about([10.0, 20.0], [0.0, 0.0], [2.0, 3.0], 30.0);
        let p = [
            m[0] * 10.0 + m[1] * 20.0 + m[2],
            m[3] * 10.0 + m[4] * 20.0 + m[5],
        ];
        assert!((p[0] - 10.0).abs() < 1e-4 && (p[1] - 20.0).abs() < 1e-4);
    }
}
