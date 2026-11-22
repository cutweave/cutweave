//! Independent CPU reference renderer (`f64` throughout).
//!
//! Walks the [`Scene`] directly (it does not use the render graph or any shader code) and
//! implements the same documented semantics: MPEG-2-sited bilinear chroma, H.273 ranges and
//! matrices, transfer functions, BT.2020→709 primaries, BT.2390 EETF on max(R,G,B), texel-centre
//! bilinear layer sampling with ½-px edge coverage, W3C separable blend formulas on premultiplied
//! colour, trilinear LUT lookup, and round-half-up output quantisation. Its output is the
//! independent reference for the PSNR > 45 dB / ΔE2000 < 2 checks (WP-0F.01).

use crate::color::{self, Eetf, kr_kb, primaries_conversion, range_params};
use crate::graph::blur_weights;
use crate::image::Image;
use crate::lut::Lut3d;
use crate::patterns::rounded_box_sdf;
use crate::scene::{
    BlendMode, ColorTags, Effect, Layer, Matrix, Node, OutputTransform, Primaries, Scene, Transfer,
    apply_affine,
};
use crate::source::{PixelData, SourceFrame, SourceProvider};
use crate::{Error, Result};

type Px = [f64; 4];

/// Premultiplied linear image.
#[derive(Clone, Debug)]
pub struct LinearImage {
    pub w: u32,
    pub h: u32,
    pub px: Vec<Px>,
}

impl LinearImage {
    fn filled(w: u32, h: u32, c: Px) -> Self {
        Self {
            w,
            h,
            px: vec![c; (w * h) as usize],
        }
    }

    fn at(&self, x: i64, y: i64) -> Px {
        let x = x.clamp(0, i64::from(self.w) - 1);
        let y = y.clamp(0, i64::from(self.h) - 1);
        self.px[(y * i64::from(self.w) + x) as usize]
    }
}

fn lerp4(a: Px, b: Px, t: f64) -> Px {
    [0, 1, 2, 3].map(|i| a[i] * (1.0 - t) + b[i] * t)
}

/// Renders `scene` to an encoded output image.
pub fn render(scene: &Scene, provider: &dyn SourceProvider) -> Result<Image> {
    let lin = render_linear(scene, provider)?;
    Ok(encode_output(&lin, &scene.output))
}

/// Renders `scene` to the linear working space (before the output transform).
pub fn render_linear(scene: &Scene, provider: &dyn SourceProvider) -> Result<LinearImage> {
    scene.validate()?;
    let bg = scene.background.map(f64::from);
    let mut img = LinearImage::filled(scene.width, scene.height, bg);
    for layer in &scene.layers {
        composite_layer(&mut img, layer, provider)?;
    }
    for e in &scene.effects {
        img = apply_effect(img, e, provider)?;
    }
    Ok(img)
}

/// Linear → output encoding and quantisation.
#[must_use]
pub fn encode_output(img: &LinearImage, out: &OutputTransform) -> Image {
    let max = if out.bit_depth == 16 { 65535.0 } else { 255.0 };
    let q = |v: f64| (v * max + 0.5).floor().clamp(0.0, max) as u16;
    let mut data = Vec::with_capacity(img.px.len() * 4);
    for p in &img.px {
        for c in &p[..3] {
            data.push(q(color::from_linear(c.clamp(0.0, 1.0), out.transfer)));
        }
        data.push(q(p[3].clamp(0.0, 1.0)));
    }
    Image {
        width: img.w,
        height: img.h,
        bit_depth: out.bit_depth,
        data,
    }
}

fn composite_layer(
    canvas: &mut LinearImage,
    layer: &Layer,
    provider: &dyn SourceProvider,
) -> Result<()> {
    if layer.opacity.is_nan() || layer.opacity <= 0.0 {
        return Ok(());
    }
    if let Node::Text(t) = &layer.content {
        // Same semantics as the graph: glyphs rasterised through the layer transform at canvas
        // resolution (no resampling), effects in canvas pixels, placed without a transform.
        let box_size = [f64::from(t.box_size[0]), f64::from(t.box_size[1])];
        let target = layer.transform.forward(box_size);
        let mut content = render_text(t, &target, (canvas.w, canvas.h))?;
        for e in &layer.effects {
            content = apply_effect(content, e, provider)?;
        }
        let opacity = f64::from(layer.opacity.min(1.0));
        for (d, s) in canvas.px.iter_mut().zip(&content.px) {
            *d = blend(*d, s.map(|v| v * opacity), layer.blend);
        }
        return Ok(());
    }
    let mut content = render_node(&layer.content, (canvas.w, canvas.h), provider)?;
    for e in &layer.effects {
        content = apply_effect(content, e, provider)?;
    }
    let (lw, lh) = (f64::from(content.w), f64::from(content.h));
    let Some(inv) = layer.transform.inverse([lw, lh]) else {
        return Ok(());
    };
    let opacity = f64::from(layer.opacity.min(1.0));
    for y in 0..canvas.h {
        for x in 0..canvas.w {
            let u = apply_affine(&inv, [f64::from(x) + 0.5, f64::from(y) + 0.5]);
            let cov = ((u[0].min(lw - u[0]) + 0.5).clamp(0.0, 1.0))
                * ((u[1].min(lh - u[1]) + 0.5).clamp(0.0, 1.0));
            let src = if cov > 0.0 {
                let s = sample_bilinear(&content, u);
                s.map(|v| v * (cov * opacity))
            } else {
                [0.0; 4]
            };
            let i = (y * canvas.w + x) as usize;
            canvas.px[i] = blend(canvas.px[i], src, layer.blend);
        }
    }
    Ok(())
}

/// Bilinear sample at layer-pixel position `u` (texel centres at +0.5), clamp-to-edge.
fn sample_bilinear(img: &LinearImage, u: [f64; 2]) -> Px {
    let (px, py) = (u[0] - 0.5, u[1] - 0.5);
    let (x0, y0) = (px.floor(), py.floor());
    let (fx, fy) = (px - x0, py - y0);
    let (xi, yi) = (x0 as i64, y0 as i64);
    let top = lerp4(img.at(xi, yi), img.at(xi + 1, yi), fx);
    let bottom = lerp4(img.at(xi, yi + 1), img.at(xi + 1, yi + 1), fx);
    lerp4(top, bottom, fy)
}

fn blend_channel(cb: f64, cs: f64, mode: BlendMode) -> f64 {
    let unit = |v: f64| v.clamp(0.0, 1.0);
    let screen = |b: f64, s: f64| b + s - b * s;
    match mode {
        BlendMode::Normal => cs,
        BlendMode::Multiply => cb * cs,
        BlendMode::Screen => screen(unit(cb), unit(cs)),
        BlendMode::Overlay => {
            // Overlay = hard-light with the operands swapped.
            let (b, s) = (unit(cb), unit(cs));
            if b <= 0.5 {
                s * 2.0 * b
            } else {
                screen(s, 2.0 * b - 1.0)
            }
        }
        BlendMode::Darken => cb.min(cs),
        BlendMode::Lighten => cb.max(cs),
        BlendMode::Add => cb + cs,
        BlendMode::SoftLight => {
            let (b, s) = (unit(cb), unit(cs));
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 {
                    ((16.0 * b - 12.0) * b + 4.0) * b
                } else {
                    b.sqrt()
                };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }
    }
}

/// W3C compositing (source-over with a separable blend function), premultiplied in and out.
fn blend(dst: Px, src: Px, mode: BlendMode) -> Px {
    let (sa, da) = (src[3], dst[3]);
    if mode == BlendMode::Normal {
        return [0, 1, 2, 3].map(|i| src[i] + dst[i] * (1.0 - sa));
    }
    let mut out = [0.0; 4];
    for i in 0..3 {
        let cs = if sa > 0.0 { src[i] / sa.max(1e-8) } else { 0.0 };
        let cb = if da > 0.0 { dst[i] / da.max(1e-8) } else { 0.0 };
        let b = blend_channel(cb, cs, mode);
        out[i] = src[i] * (1.0 - da) + dst[i] * (1.0 - sa) + sa * da * b;
    }
    out[3] = sa + da * (1.0 - sa);
    out
}

fn render_node(
    node: &Node,
    canvas: (u32, u32),
    provider: &dyn SourceProvider,
) -> Result<LinearImage> {
    match node {
        Node::Media(m) => {
            let frame = provider.frame(&m.source, m.frame)?;
            let tags = m.tags.unwrap_or(frame.tags);
            let tm = (tags.transfer == Transfer::Pq).then(|| m.tone_map.unwrap_or_default());
            decode_frame(&frame, tags, tm.map(|t| Eetf::new(&t)))
        }
        Node::Shape(s) => {
            let (w, h) = (
                s.width.ceil().max(1.0) as u32,
                s.height.ceil().max(1.0) as u32,
            );
            let a = f64::from(s.fill_srgb[3].clamp(0.0, 1.0));
            let lin = |v: f32| color::to_linear(f64::from(v.clamp(0.0, 1.0)), Transfer::Srgb) * a;
            let c = [
                lin(s.fill_srgb[0]),
                lin(s.fill_srgb[1]),
                lin(s.fill_srgb[2]),
                a,
            ];
            let half = [f64::from(w) / 2.0, f64::from(h) / 2.0];
            let r = f64::from(s.corner_radius.max(0.0))
                .min(half[0])
                .min(half[1]);
            let mut img = LinearImage::filled(w, h, [0.0; 4]);
            for y in 0..h {
                for x in 0..w {
                    let p = [f64::from(x) + 0.5 - half[0], f64::from(y) + 0.5 - half[1]];
                    let cov = (0.5 - rounded_box_sdf(p, half, r)).clamp(0.0, 1.0);
                    img.px[(y * w + x) as usize] = c.map(|v| v * cov);
                }
            }
            Ok(img)
        }
        Node::Group(g) => {
            let mut img = LinearImage::filled(g.width.max(1), g.height.max(1), [0.0; 4]);
            for layer in &g.layers {
                composite_layer(&mut img, layer, provider)?;
            }
            Ok(img)
        }
        Node::Transition(t) => {
            let p = if t.progress.is_nan() {
                0.0
            } else {
                f64::from(t.progress.clamp(0.0, 1.0))
            };
            let side = |l: &Layer| -> Result<LinearImage> {
                let mut img = LinearImage::filled(canvas.0, canvas.1, [0.0; 4]);
                composite_layer(&mut img, l, provider)?;
                Ok(img)
            };
            if p <= 0.0 {
                return side(&t.from);
            }
            if p >= 1.0 {
                return side(&t.to);
            }
            let (a, b) = (side(&t.from)?, side(&t.to)?);
            let px =
                a.px.iter()
                    .zip(&b.px)
                    .map(|(x, y)| lerp4(*x, *y, p))
                    .collect();
            Ok(LinearImage { w: a.w, h: a.h, px })
        }
        Node::Text(t) => {
            let size = (
                t.box_size[0].ceil().max(1.0) as u32,
                t.box_size[1].ceil().max(1.0) as u32,
            );
            render_text(t, &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0], size)
        }
    }
}

thread_local! {
    static TEXT: std::cell::RefCell<Option<crate::text::TextEngine>> =
        const { std::cell::RefCell::new(None) };
}

/// Text via the 8×8-supersampled exact-outline rasteriser (`text::raster`); fill, spans and
/// karaoke only (outline / shadow → `Unsupported`).
fn render_text(
    t: &crate::scene::TextNode,
    target: &[f64; 6],
    size: (u32, u32),
) -> Result<LinearImage> {
    TEXT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(crate::text::TextEngine::new()?);
        }
        let engine = slot
            .as_mut()
            .ok_or_else(|| Error::Font("text engine".into()))?;
        let (r, _) = engine.prepare(t);
        let px = crate::text::raster::render(engine, &r, target, size)?;
        Ok(LinearImage {
            w: size.0,
            h: size.1,
            px,
        })
    })
}

fn plane_bilinear(plane: &[u16], w: u32, h: u32, x: f64, y: f64) -> f64 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let get = |xi: i64, yi: i64| {
        let xi = xi.clamp(0, i64::from(w) - 1);
        let yi = yi.clamp(0, i64::from(h) - 1);
        f64::from(plane[(yi * i64::from(w) + xi) as usize])
    };
    let (xi, yi) = (x0 as i64, y0 as i64);
    let top = get(xi, yi) * (1.0 - fx) + get(xi + 1, yi) * fx;
    let bottom = get(xi, yi + 1) * (1.0 - fx) + get(xi + 1, yi + 1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Input colour transform of one frame → premultiplied linear Rec.709.
pub fn decode_frame(
    frame: &SourceFrame,
    tags: ColorTags,
    eetf: Option<Eetf>,
) -> Result<LinearImage> {
    let prim = primaries_conversion(tags.primaries, Primaries::Bt709);
    let scale = if tags.transfer == Transfer::Pq {
        10_000.0
    } else {
        1.0
    };
    let finish = |rgb: [f64; 3]| -> [f64; 3] {
        let lin = rgb.map(|v| color::to_linear(v.clamp(0.0, 1.0), tags.transfer) * scale);
        let mut lin = color::mat_vec(&prim, lin);
        if let Some(e) = &eetf {
            lin = e.tone_map(lin);
        }
        lin.map(|v| v.max(0.0))
    };
    let (w, h) = (frame.width, frame.height);
    let mut px = Vec::with_capacity((w * h) as usize);
    match &frame.pixels {
        PixelData::Yuv420 {
            bit_depth,
            y,
            cb,
            cr,
        } => {
            if tags.matrix == Matrix::Rgb {
                return Err(Error::Unsupported("planar 4:2:0 data tagged as RGB".into()));
            }
            let (kr, kb) = kr_kb(tags.matrix).unwrap_or((0.2126, 0.0722));
            let rp = range_params(tags.range, *bit_depth);
            let (cw, ch) = frame.chroma_size();
            for yy in 0..h {
                for xx in 0..w {
                    let (cx, cy) = (f64::from(xx) * 0.5, (f64::from(yy) - 0.5) * 0.5);
                    let yn = (f64::from(y[(yy * w + xx) as usize]) - rp.y_off) / rp.y_scale;
                    let cbn = (plane_bilinear(cb, cw, ch, cx, cy) - rp.c_off) / rp.c_scale;
                    let crn = (plane_bilinear(cr, cw, ch, cx, cy) - rp.c_off) / rp.c_scale;
                    let rgb = finish(color::ycbcr_to_rgb(yn, cbn, crn, kr, kb));
                    px.push([rgb[0], rgb[1], rgb[2], 1.0]);
                }
            }
        }
        PixelData::Rgba8(data) => {
            for p in data.as_chunks::<4>().0 {
                let a = f64::from(p[3]) / 255.0;
                let rgb = finish([p[0], p[1], p[2]].map(|v| f64::from(v) / 255.0));
                px.push([rgb[0] * a, rgb[1] * a, rgb[2] * a, a]);
            }
        }
    }
    Ok(LinearImage { w, h, px })
}

fn apply_effect(
    img: LinearImage,
    e: &Effect,
    provider: &dyn SourceProvider,
) -> Result<LinearImage> {
    match e {
        Effect::BlurLite { radius } => {
            if *radius <= 0.0 {
                return Ok(img);
            }
            let w: Vec<f64> = blur_weights(radius.min(30.0))
                .into_iter()
                .map(f64::from)
                .collect();
            let h = blur_1d(&img, &w, true);
            Ok(blur_1d(&h, &w, false))
        }
        Effect::Lut3d { lut, space, mix } => {
            let l = provider.lut(lut)?;
            Ok(apply_lut(&img, &l, *space, f64::from(mix.clamp(0.0, 1.0))))
        }
    }
}

fn blur_1d(img: &LinearImage, w: &[f64], horizontal: bool) -> LinearImage {
    let mut out = img.clone();
    for y in 0..i64::from(img.h) {
        for x in 0..i64::from(img.w) {
            let mut acc = img.at(x, y).map(|v| v * w[0]);
            for (i, wi) in w.iter().enumerate().skip(1) {
                let i = i as i64;
                let (a, b) = if horizontal {
                    (img.at(x + i, y), img.at(x - i, y))
                } else {
                    (img.at(x, y + i), img.at(x, y - i))
                };
                for c in 0..4 {
                    acc[c] += wi * (a[c] + b[c]);
                }
            }
            out.px[(y * i64::from(img.w) + x) as usize] = acc;
        }
    }
    out
}

fn apply_lut(img: &LinearImage, lut: &Lut3d, space: Transfer, mix: f64) -> LinearImage {
    let px = img
        .px
        .iter()
        .map(|c| {
            let a = c[3];
            if a <= 0.0 {
                return *c;
            }
            let rgb = [c[0], c[1], c[2]].map(|v| (v / a).clamp(0.0, 1.0));
            let enc = rgb.map(|v| color::from_linear(v, space));
            let lin = lut.sample(enc).map(|v| color::to_linear(v, space));
            let o = [0, 1, 2].map(|i| (rgb[i] * (1.0 - mix) + lin[i] * mix) * a);
            [o[0], o[1], o[2], a]
        })
        .collect();
    LinearImage {
        w: img.w,
        h: img.h,
        px,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SyntheticSources;
    use crate::timeline::{test_scenes, text_scene};

    #[test]
    fn blend_formulas_on_opaque_inputs() {
        let dst = [0.25, 0.5, 0.75, 1.0];
        let src = [0.5, 0.5, 0.5, 1.0];
        assert_eq!(blend(dst, src, BlendMode::Normal), src);
        assert_eq!(
            blend(dst, src, BlendMode::Multiply),
            [0.125, 0.25, 0.375, 1.0]
        );
        assert_eq!(
            blend(dst, src, BlendMode::Screen),
            [0.625, 0.75, 0.875, 1.0]
        );
        assert_eq!(blend(dst, src, BlendMode::Darken), [0.25, 0.5, 0.5, 1.0]);
        assert_eq!(blend(dst, src, BlendMode::Add), [0.75, 1.0, 1.25, 1.0]);
        // Soft-light with a 50% source is the identity.
        assert_eq!(blend(dst, src, BlendMode::SoftLight), dst);
    }

    #[test]
    fn zero_opacity_source_leaves_backdrop_untouched() {
        let dst = [0.2, 0.3, 0.4, 1.0];
        for m in BlendMode::ALL {
            assert_eq!(blend(dst, [0.0; 4], m), dst, "{m:?}");
        }
    }

    #[test]
    fn identity_scene_round_trips_bars() {
        // 75% bars through BT.709 in → linear → BT.709 out must give back 75% (191) codes.
        let p = SyntheticSources::new(64, 36).expect("sources");
        let (_, scene) = test_scenes(64, 36)
            .into_iter()
            .find(|(n, _)| n == "identity709")
            .expect("scene");
        let img = render(&scene, &p).expect("render");
        let px = img.pixel(4, 4); // inside the first (75% white) bar
        for c in &px[..3] {
            assert!((i32::from(*c) - 191).abs() <= 1, "{px:?}");
        }
    }

    #[test]
    fn text_renders_fill_and_rejects_outline_in_the_reference() {
        let p = SyntheticSources::new(320, 180).expect("sources");
        let mut s = text_scene(320, 180);
        s.layers.truncate(1);
        s.effects.clear();
        let plain = {
            let mut s = s.clone();
            s.layers.clear();
            render(&s, &p).expect("no text")
        };
        let mut with_text = s.clone();
        with_text.layers.clear();
        with_text
            .layers
            .push(text_scene(320, 180).layers[3].clone());
        let img = render(&with_text, &p).expect("text");
        assert!(img.diff_count(&plain) > 50, "glyphs were drawn");
        if let Node::Text(t) = &mut with_text.layers[0].content {
            t.outline = Some(crate::scene::TextOutline {
                width_px: 2.0,
                color_srgb: [0.0, 0.0, 0.0, 1.0],
            });
        }
        assert!(matches!(render(&with_text, &p), Err(Error::Unsupported(_))));
    }
}
