//! Vello variant: the same [`ResolvedText`] encoded as a `vello::Scene` (glyph runs grouped by
//! face/size/transform/brush; outline = round-join stroke under the fill; karaoke = hard-stop
//! linear gradient in layout space). Colours are the linear premultiplied values un-premultiplied
//! and handed to Vello as if they were sRGB, so Vello's numeric blending happens in linear light
//! (its RGBA8 target is then 8-bit *linear*, see the README findings).
//!
//! Not supported here (findings): soft shadows (Vello 0.12 has no path blur; the shadow is
//! hard-edged). The box clip is a clip layer.

use std::collections::HashMap;
use std::sync::Arc;

use vello::kurbo::{Affine, Join, Rect, Stroke};
use vello::peniko::{Blob, Brush, Color, ColorStop, Fill, FontData, Gradient};
use vello::{Glyph, Scene};

use super::fonts::FontLibrary;
use super::outline::normalized_coords;
use super::{Highlight, ResolvedGlyph, ResolvedText, compose};

/// Per-face Vello font handles (a stable `Blob` id keeps Vello's glyph cache warm).
#[derive(Default)]
pub struct VelloFonts {
    fonts: HashMap<u64, FontData>,
    coords: HashMap<(u64, u16), Arc<Vec<i16>>>,
}

impl std::fmt::Debug for VelloFonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VelloFonts")
            .field("fonts", &self.fonts.len())
            .finish()
    }
}

fn kurbo(m: &[f32; 6]) -> Affine {
    Affine::new([
        f64::from(m[0]),
        f64::from(m[3]),
        f64::from(m[1]),
        f64::from(m[4]),
        f64::from(m[2]),
        f64::from(m[5]),
    ])
}

fn color(premul: [f32; 4], opacity: f32) -> Color {
    let a = premul[3];
    let un = |c: f32| if a > 0.0 { c / a } else { 0.0 };
    Color::new([un(premul[0]), un(premul[1]), un(premul[2]), a * opacity])
}

#[derive(Clone, Copy, PartialEq)]
enum Layer {
    Shadow,
    Outline,
    Fill,
}

struct Run<'a> {
    font: &'a FontData,
    coords: &'a [i16],
    size: f32,
    m: Affine,
    glyphs: &'a [Glyph],
}

impl Run<'_> {
    fn fill(&self, scene: &mut Scene, brush: &Brush) {
        scene
            .draw_glyphs(self.font)
            .font_size(self.size)
            .transform(self.m)
            .normalized_coords(self.coords)
            .brush(brush)
            .draw(Fill::NonZero, self.glyphs.iter().copied());
    }

    fn stroke(&self, scene: &mut Scene, brush: &Brush, width: f32) {
        let stroke = Stroke::new(f64::from(2.0 * width)).with_join(Join::Round);
        scene
            .draw_glyphs(self.font)
            .font_size(self.size)
            .transform(self.m)
            .normalized_coords(self.coords)
            .brush(brush)
            .draw(&stroke, self.glyphs.iter().copied());
    }
}

impl VelloFonts {
    fn font(&mut self, lib: &FontLibrary, g: &ResolvedGlyph) -> Option<(FontData, Arc<Vec<i16>>)> {
        let face = lib.face(g.face)?;
        let font = self
            .fonts
            .entry(face.content_hash)
            .or_insert_with(|| {
                let blob = Blob::new(Arc::new(face.data.as_ref().clone()));
                FontData::new(blob, face.index)
            })
            .clone();
        let coords = self
            .coords
            .entry((face.content_hash, g.key.weight))
            .or_insert_with(|| Arc::new(normalized_coords(face, g.key.weight)))
            .clone();
        Some((font, coords))
    }

    /// Encodes `r` with `target` (box px → target texture px) into `scene`.
    pub fn encode(
        &mut self,
        lib: &FontLibrary,
        r: &ResolvedText,
        target: &[f32; 6],
        scene: &mut Scene,
    ) {
        let clip = Rect::new(0.0, 0.0, f64::from(r.box_size[0]), f64::from(r.box_size[1]));
        scene.push_clip_layer(Fill::NonZero, kurbo(target), &clip);
        if let Some(s) = r.shadow {
            let shifted = compose(target, &[1.0, 0.0, s.offset[0], 0.0, 1.0, s.offset[1]]);
            self.layer(lib, r, &shifted, Layer::Shadow, scene);
        }
        if r.outline.is_some() {
            self.layer(lib, r, target, Layer::Outline, scene);
        }
        self.layer(lib, r, target, Layer::Fill, scene);
        scene.pop_layer();
    }

    fn layer(
        &mut self,
        lib: &FontLibrary,
        r: &ResolvedText,
        target: &[f32; 6],
        layer: Layer,
        scene: &mut Scene,
    ) {
        let mut glyphs = Vec::new();
        let mut i = 0;
        while i < r.glyphs.len() {
            // A run: consecutive glyphs sharing face, weight, size, transform and style.
            let g0 = r.glyphs[i];
            let mut j = i + 1;
            while j < r.glyphs.len() {
                let g = &r.glyphs[j];
                if g.face != g0.face
                    || g.key.weight != g0.key.weight
                    || g.size != g0.size
                    || g.anim != g0.anim
                    || g.opacity != g0.opacity
                    || g.fill != g0.fill
                    || g.highlight != g0.highlight
                {
                    break;
                }
                j += 1;
            }
            let span = &r.glyphs[i..j];
            i = j;
            let Some((font, coords)) = self.font(lib, &g0) else {
                continue;
            };
            glyphs.clear();
            glyphs.extend(span.iter().map(|g| Glyph {
                id: u32::from(g.key.glyph),
                x: g.pen[0],
                y: g.pen[1],
            }));
            let run = Run {
                font: &font,
                coords: &coords,
                size: g0.size,
                m: kurbo(&compose(target, &g0.anim)),
                glyphs: &glyphs,
            };
            match layer {
                Layer::Shadow => {
                    let Some(s) = r.shadow else { return };
                    let brush = Brush::Solid(color(s.color, g0.opacity));
                    if let Some((w, _)) = r.outline {
                        run.stroke(scene, &brush, w);
                    }
                    run.fill(scene, &brush);
                }
                Layer::Outline => {
                    let Some((w, c)) = r.outline else { return };
                    run.stroke(scene, &Brush::Solid(color(c, g0.opacity)), w);
                }
                Layer::Fill => {
                    let brush = match g0.highlight {
                        None => Brush::Solid(color(g0.fill, g0.opacity)),
                        Some(h) => sweep_gradient(span, &g0, h),
                    };
                    run.fill(scene, &brush);
                }
            }
        }
    }
}

/// Hard-stop gradient across the run in layout space (the run transform applies to brushes).
fn sweep_gradient(run: &[ResolvedGlyph], g0: &ResolvedGlyph, h: Highlight) -> Brush {
    let x0 = run.iter().map(|g| g.pen[0]).fold(f32::MAX, f32::min) - g0.size;
    let x1 = run
        .iter()
        .map(|g| g.pen[0] + g.advance)
        .fold(f32::MIN, f32::max)
        + g0.size;
    let t = ((h.sweep_x - x0) / (x1 - x0)).clamp(0.0, 1.0);
    let (hl, base) = (color(h.color, g0.opacity), color(g0.fill, g0.opacity));
    let (left, right) = if h.dir > 0.0 { (hl, base) } else { (base, hl) };
    let stops = [
        ColorStop::from((0.0, left)),
        ColorStop::from((t, left)),
        ColorStop::from((t, right)),
        ColorStop::from((1.0, right)),
    ];
    let y = f64::from(g0.pen[1]);
    Gradient::new_linear((f64::from(x0), y), (f64::from(x1), y))
        .with_stops(stops)
        .into()
}
