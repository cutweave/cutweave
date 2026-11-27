// SPDX-License-Identifier: GPL-3.0-or-later
//! The independent CPU reference compositor, in `f64`.
//!
//! It defines the compositor's semantics executably and checks the GPU path (arch §20; WP-1F.02
//! acceptance: GPU vs CPU float reference, PSNR > 45 dB and ΔE2000 < 2). It walks the
//! *scene* (not the render graph, so culling and decode sharing are checked too) and implements
//! the documented rules without sharing code with the WGSL shaders; only the colour constants
//! ([`crate::color`]), the layer placement ([`crate::geometry`], unit-tested on its own, flip
//! included) and the analytic blend formulas ([`crate::BlendMode::composite`]) are common.
//!
//! Semantics, per output pixel centre `p`:
//!
//! 1. **Decode** every source pixel: codes → R'G'B' (4:2:0 chroma bilinearly upsampled at its
//!    siting, clamp-to-edge) → linear Rec.709 → premultiplied by alpha.
//! 2. **Mips** when minifying: level `L` is the 2×2 box average of level `L−1` (edges clamped),
//!    size `max(1, size >> L)`.
//! 3. **Sample** at `t = to_texel(from_output(p))` with exact-weight bilinear filtering
//!    (`a + (b − a)·f`, texel centres at +0.5, clamp-to-edge), or Catmull-Rom bicubic over 4×4
//!    texels (rows first, then columns; clamped to `rgb ≥ 0`, `a ∈ [0, 1]`) with
//!    [`Filter::Bicubic`]; trilinear between `floor(lod)` and the next level when `lod > 0`.
//! 4. **Coverage** × opacity scales the sample; the layer's [`crate::BlendMode`] composites it
//!    onto the canvas (normal: **over**, `dst = src + dst·(1 − src.a)`; premultiplied, linear
//!    light).
//! 5. **Output**: un-premultiply, clamp, encode, round half up.
//!
//! Before the layers, the **background fill** ([`crate::backdrop`], WP-2B.02): the canvas starts
//! as the colour (transparent black for a transparent background); an image is drawn over it
//! like a layer; a blur draws its layer's picture, covering, onto the small canvas, blurs it
//! there (two Gaussian passes, then the dim) and stretches the result over the canvas with the
//! bilinear rule of step 3. A frame with a blur quantises with the ordered dither. Only the
//! plan ([`crate::backdrop::blur_plan`]), the synthetic layers and the dither matrix are shared
//! with the GPU path.
//!
//! The GPU stores intermediates as `Rgba16Float`; the reference does not round them, which
//! bounds agreement at about one 8-bit code, or about `1e-3` in linear light
//! ([`render_linear`]).

// Pixel and matrix math reads best with the conventional single-letter names.
#![allow(clippy::many_single_char_names)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use crate::backdrop::{blur_plan, cover_layer, dither_threshold, gaussian_weight, image_layer};
use crate::color::{
    InputColor, InputTransform, encode_output, encode_output_dithered, merge_tags,
    srgb8_to_linear_premul,
};
use crate::frame::{FrameError, FrameSource, PixelFormat, VideoFrame};
use crate::geometry::{LayerGeometry, apply_affine, mip_levels_for, mip_size};
use crate::graph::{Filter, GraphError, GraphOptions, Target};
use crate::image::Image;
use crate::scene::{BackgroundNode, BlendMode, Layer, SceneDescription};

type Rgba = [f64; 4];

/// A linear premultiplied image level.
struct Level {
    w: u32,
    h: u32,
    px: Vec<Rgba>,
}

impl Level {
    fn at(&self, x: i64, y: i64) -> Rgba {
        let cx = x.clamp(0, i64::from(self.w) - 1) as usize;
        let cy = y.clamp(0, i64::from(self.h) - 1) as usize;
        self.px[cy * self.w as usize + cx]
    }

    fn bilinear(&self, t: [f64; 2]) -> Rgba {
        let p = [t[0] - 0.5, t[1] - 0.5];
        let f0 = [p[0].floor(), p[1].floor()];
        let fr = [p[0] - f0[0], p[1] - f0[1]];
        let (x, y) = (f0[0] as i64, f0[1] as i64);
        let (a, b, c, d) = (
            self.at(x, y),
            self.at(x + 1, y),
            self.at(x, y + 1),
            self.at(x + 1, y + 1),
        );
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fr[0];
            let bottom = c[i] + (d[i] - c[i]) * fr[0];
            out[i] = top + (bottom - top) * fr[1];
        }
        out
    }

    /// Catmull-Rom bicubic: each of the 4 rows weighted across, then the rows down; clamped
    /// to no negative light and alpha in `[0, 1]`.
    fn bicubic(&self, t: [f64; 2]) -> Rgba {
        let p = [t[0] - 0.5, t[1] - 0.5];
        let f0 = [p[0].floor(), p[1].floor()];
        let (wx, wy) = (cubic_weights(p[0] - f0[0]), cubic_weights(p[1] - f0[1]));
        let (x, y) = (f0[0] as i64, f0[1] as i64);
        let mut acc = [0.0; 4];
        for (j, wj) in (-1..=2).zip(wy) {
            let mut row = [0.0; 4];
            for (i, wi) in (-1..=2).zip(wx) {
                let s = self.at(x + i, y + j);
                for c in 0..4 {
                    row[c] += s[c] * wi;
                }
            }
            for c in 0..4 {
                acc[c] += row[c] * wj;
            }
        }
        [
            acc[0].max(0.0),
            acc[1].max(0.0),
            acc[2].max(0.0),
            acc[3].clamp(0.0, 1.0),
        ]
    }

    fn filtered(&self, t: [f64; 2], filter: Filter) -> Rgba {
        match filter {
            Filter::Bicubic => self.bicubic(t),
            _ => self.bilinear(t),
        }
    }

    fn downsample(&self) -> Level {
        let (w, h) = mip_size(self.w, self.h, 1);
        let mut px = Vec::with_capacity(w as usize * h as usize);
        for y in 0..i64::from(h) {
            for x in 0..i64::from(w) {
                let (a, b, c, d) = (
                    self.at(2 * x, 2 * y),
                    self.at(2 * x + 1, 2 * y),
                    self.at(2 * x, 2 * y + 1),
                    self.at(2 * x + 1, 2 * y + 1),
                );
                px.push(std::array::from_fn(|i| (a[i] + b[i] + c[i] + d[i]) * 0.25));
            }
        }
        Level { w, h, px }
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Bilinear chroma (or any plane) sample at fractional sample position `pos`, clamp-to-edge.
fn plane_bilinear(frame: &VideoFrame, plane: usize, comp: usize, pos: [f64; 2]) -> f64 {
    let f0 = [pos[0].floor(), pos[1].floor()];
    let fr = [pos[0] - f0[0], pos[1] - f0[1]];
    let (x, y) = (f0[0] as i64, f0[1] as i64);
    let c = |dx: i64, dy: i64| f64::from(frame.code(plane, comp, x + dx, y + dy));
    let top = lerp(c(0, 0), c(1, 0), fr[0]);
    let bottom = lerp(c(0, 1), c(1, 1), fr[0]);
    lerp(top, bottom, fr[1])
}

/// Decodes a frame to premultiplied linear Rec.709 (level 0).
fn decode(frame: &VideoFrame, it: &InputTransform) -> Level {
    let (w, h) = (frame.width, frame.height);
    let off = frame.chroma_siting.offset();
    let mut px = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        for x in 0..w {
            let (xi, yi) = (i64::from(x), i64::from(y));
            let rgba = if frame.format.is_yuv() {
                let pos = [(f64::from(x) - off[0]) * 0.5, (f64::from(y) - off[1]) * 0.5];
                let (cb, cr) = match frame.format {
                    PixelFormat::Yuv420p => (
                        plane_bilinear(frame, 1, 0, pos),
                        plane_bilinear(frame, 2, 0, pos),
                    ),
                    _ => (
                        plane_bilinear(frame, 1, 0, pos),
                        plane_bilinear(frame, 1, 1, pos),
                    ),
                };
                let yc = f64::from(frame.code(0, 0, xi, yi));
                let lin = it.signal_to_linear(it.codes_to_signal(yc, cb, cr));
                [lin[0], lin[1], lin[2], 1.0]
            } else {
                let c = |i: usize| f64::from(frame.code(0, i, xi, yi));
                let lin = it.signal_to_linear(it.codes_to_signal(c(0), c(1), c(2)));
                let a = c(3) / 255.0;
                [lin[0] * a, lin[1] * a, lin[2] * a, a]
            };
            px.push(rgba);
        }
    }
    Level { w, h, px }
}

/// The Catmull-Rom (cubic convolution, `a = −½`) weights of the texels at offsets −1, 0, 1, 2
/// from the one left of (above) a sample at fraction `f`.
fn cubic_weights(f: f64) -> [f64; 4] {
    let (f2, f3) = (f * f, f * f * f);
    [
        f2 - 0.5 * f3 - 0.5 * f,
        1.5 * f3 - 2.5 * f2 + 1.0,
        2.0 * f2 - 1.5 * f3 + 0.5 * f,
        0.5 * f3 - 0.5 * f2,
    ]
}

fn sample(levels: &[Level], t: [f64; 2], lod: f64, filter: Filter) -> Rgba {
    let base = &levels[0];
    let at_level = |l: usize| {
        let lv = &levels[l];
        let s = [
            f64::from(lv.w) / f64::from(base.w),
            f64::from(lv.h) / f64::from(base.h),
        ];
        lv.filtered([t[0] * s[0], t[1] * s[1]], filter)
    };
    if lod <= 0.0 {
        return base.filtered(t, filter);
    }
    let top = (levels.len() - 1) as f64;
    let l0 = lod.floor().min(top);
    let frac = if lod.floor() >= top {
        0.0
    } else {
        lod - lod.floor()
    };
    let s0 = at_level(l0 as usize);
    if frac == 0.0 {
        return s0;
    }
    let s1 = at_level((l0 as usize + 1).min(levels.len() - 1));
    std::array::from_fn(|i| lerp(s0[i], s1[i], frac))
}

/// Renders `scene` for an RGBA8 target on the CPU. Layers whose frame is unavailable are skipped
/// and layers whose media is offline show the slate (as on the GPU). `Texture` targets are rendered as sRGB RGBA8.
pub fn render(
    scene: &SceneDescription,
    target: Target,
    source: &dyn FrameSource,
    options: &GraphOptions,
) -> Result<Image, GraphError> {
    let (w, h) = target.size();
    let transfer = match target {
        Target::Rgba8Cpu { transfer, .. } => transfer,
        Target::Texture { .. } => crate::color::OutputTransfer::Srgb,
    };
    let canvas = render_linear(scene, (w, h), source, options)?;
    let dither = dithers(scene, (w, h), source);
    let mut img = Image::filled(w, h, [0; 4]);
    for (i, (px, c)) in img
        .data
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(&canvas)
        .enumerate()
    {
        px.copy_from_slice(&if dither {
            let (x, y) = ((i % w as usize) as u32, (i / w as usize) as u32);
            encode_output_dithered(*c, transfer, dither_threshold(x, y))
        } else {
            encode_output(*c, transfer)
        });
    }
    Ok(img)
}

/// The frame of `layer` as the compositor shows it: its picture, the slate when the media is
/// offline, nothing when it is unavailable.
fn frame_of(layer: &Layer, source: &dyn FrameSource) -> Option<std::sync::Arc<VideoFrame>> {
    match source.frame(&layer.request) {
        Ok(frame) => Some(frame),
        // The file is gone: the "media offline" slate, as on the GPU.
        Err(FrameError::Offline(_)) => Some(crate::slate::frame()),
        Err(_) => None,
    }
}

/// Whether `layer` hides everything below it on an `out`-sized frame: opaque content, normal
/// blend, full opacity, covering every pixel (the rule the graph culls by).
fn hides_below(
    layer: &Layer,
    canvas: (u32, u32),
    out: (u32, u32),
    source: &dyn FrameSource,
) -> bool {
    if layer.blend != BlendMode::Normal || layer.opacity.is_nan() || layer.opacity < 1.0 {
        return false;
    }
    let Some(frame) = frame_of(layer, source) else {
        return false;
    };
    frame.format.is_opaque()
        && LayerGeometry::new(layer, canvas, out, (frame.width, frame.height))
            .is_some_and(|geo| geo.covers(out))
}

/// Whether the frame quantises with the ordered dither: it has a blurred background that is
/// drawn (one under an opaque cover is not, and changes no pixel).
fn dithers(scene: &SceneDescription, out: (u32, u32), source: &dyn FrameSource) -> bool {
    let Some(BackgroundNode::Blur { layer, .. }) = scene.background_node else {
        return false;
    };
    let canvas = (scene.width, scene.height);
    scene
        .layers
        .get(layer)
        .is_some_and(|l| frame_of(l, source).is_some())
        && !scene
            .layers
            .iter()
            .any(|l| hides_below(l, canvas, out, source))
}

/// Draws `layer` showing `frame` onto `canvas` (`out`-sized, row by row), a `scene`-sized
/// frame scaled to it: steps 1 to 4 of the module docs.
fn draw_layer(
    canvas: &mut [Rgba],
    out: (u32, u32),
    scene: (u32, u32),
    layer: &Layer,
    frame: &VideoFrame,
    options: &GraphOptions,
) {
    let (w, h) = out;
    let Some(geo) = LayerGeometry::new(layer, scene, out, (frame.width, frame.height)) else {
        return;
    };
    let height = layer.request.source_size.map_or(frame.height, |s| s.1);
    let color = InputColor::resolve(
        &merge_tags(&frame.color, &layer.request.color),
        frame.format.is_yuv(),
        height,
    );
    let it = InputTransform::new(&color, frame.format.bit_depth(), &options.tone_map);
    let lod = geo.lod();
    let mut levels = vec![decode(frame, &it)];
    for _ in 1..mip_levels_for(lod, frame.width, frame.height) {
        let next = levels[levels.len() - 1].downsample();
        levels.push(next);
    }
    let opacity = layer.opacity.min(1.0);
    for y in 0..h {
        for x in 0..w {
            let p = [f64::from(x) + 0.5, f64::from(y) + 0.5];
            let cov = geo.coverage(p);
            if cov <= 0.0 {
                continue;
            }
            let t = apply_affine(&geo.to_texel, apply_affine(&geo.from_output, p));
            let s = sample(&levels, t, lod, options.filter).map(|c| c * cov * opacity);
            let d = &mut canvas[y as usize * w as usize + x as usize];
            *d = layer.blend.composite(s, *d);
        }
    }
}

/// One Gaussian pass over `src` along x (`horizontal`) or y: weights `exp(−i² / 2σ²)` for `i`
/// in `−taps..=taps`, normalised, edges clamped; then the colour times `1 − dim`.
fn blur_pass(src: &Level, horizontal: bool, sigma: f64, taps: u32, dim: f64) -> Level {
    let taps = taps as i32;
    let weights: Vec<f64> = (-taps..=taps)
        .map(|i| {
            if taps == 0 {
                1.0
            } else {
                gaussian_weight(i, sigma)
            }
        })
        .collect();
    let total: f64 = weights.iter().sum();
    let keep = 1.0 - dim;
    let mut px = Vec::with_capacity(src.px.len());
    for y in 0..i64::from(src.h) {
        for x in 0..i64::from(src.w) {
            let mut acc = [0.0; 4];
            for (i, wgt) in (-taps..=taps).zip(&weights) {
                let i = i64::from(i);
                let s = if horizontal {
                    src.at(x + i, y)
                } else {
                    src.at(x, y + i)
                };
                for c in 0..4 {
                    acc[c] += s[c] * wgt;
                }
            }
            px.push([
                acc[0] / total * keep,
                acc[1] / total * keep,
                acc[2] / total * keep,
                acc[3] / total,
            ]);
        }
    }
    Level {
        w: src.w,
        h: src.h,
        px,
    }
}

/// Draws the scene's background fill over `canvas` (already the colour).
fn draw_fill(
    canvas: &mut [Rgba],
    bg: Rgba,
    scene: &SceneDescription,
    (w, h): (u32, u32),
    source: &dyn FrameSource,
    options: &GraphOptions,
) {
    let size = (scene.width, scene.height);
    match &scene.background_node {
        None | Some(BackgroundNode::Transparent) => {}
        Some(BackgroundNode::Image { request, fit }) => {
            let Ok(frame) = source.frame(request) else {
                return;
            };
            let layer = image_layer(request, *fit, (frame.width, frame.height), size);
            draw_layer(canvas, (w, h), size, &layer, &frame, options);
        }
        Some(BackgroundNode::Blur { layer, radius, dim }) => {
            let Some(blurred) = scene.layers.get(*layer) else {
                return;
            };
            let Some(frame) = frame_of(blurred, source) else {
                return;
            };
            let plan = blur_plan(*radius, (w, h));
            let cover = cover_layer(blurred, (frame.width, frame.height), size);
            let mut small = vec![bg; plan.size.0 as usize * plan.size.1 as usize];
            draw_layer(&mut small, plan.size, size, &cover, &frame, options);
            let small = Level {
                w: plan.size.0,
                h: plan.size.1,
                px: small,
            };
            let across = blur_pass(&small, true, plan.sigma[0], plan.taps[0], 0.0);
            let blurred = blur_pass(&across, false, plan.sigma[1], plan.taps[1], *dim);
            let scale = [
                f64::from(plan.size.0) / f64::from(w),
                f64::from(plan.size.1) / f64::from(h),
            ];
            for y in 0..h {
                for x in 0..w {
                    let t = [
                        (f64::from(x) + 0.5) * scale[0],
                        (f64::from(y) + 0.5) * scale[1],
                    ];
                    let d = &mut canvas[y as usize * w as usize + x as usize];
                    *d = BlendMode::Normal.composite(blurred.bilinear(t), *d);
                }
            }
        }
    }
}

/// Renders `scene` at `size` on the CPU into the compositor's working space: premultiplied
/// linear-light Rec.709 RGBA per pixel, row by row (what a [`Target::Texture`] holds on the
/// GPU, before any output transform). Layers whose frame is unavailable are skipped.
pub fn render_linear(
    scene: &SceneDescription,
    (w, h): (u32, u32),
    source: &dyn FrameSource,
    options: &GraphOptions,
) -> Result<Vec<[f64; 4]>, GraphError> {
    if w == 0 || h == 0 || scene.width == 0 || scene.height == 0 {
        return Err(GraphError::Empty {
            what: if w == 0 || h == 0 { "output" } else { "canvas" },
            width: w,
            height: h,
        });
    }
    let bg = match scene.background_node {
        Some(BackgroundNode::Transparent) => [0.0; 4],
        _ => srgb8_to_linear_premul(scene.background),
    };
    let mut canvas = vec![bg; w as usize * h as usize];
    draw_fill(&mut canvas, bg, scene, (w, h), source, options);
    for layer in &scene.layers {
        if layer.opacity.is_nan() || layer.opacity <= 0.0 {
            continue;
        }
        let Some(frame) = frame_of(layer, source) else {
            continue;
        };
        draw_layer(
            &mut canvas,
            (w, h),
            (scene.width, scene.height),
            layer,
            &frame,
            options,
        );
    }
    Ok(canvas)
}

/// Decodes a frame to its 8-bit R'G'B' signal (chroma upsampled, no colour conversion beyond
/// the matrix and range): "the source after YUV → RGB", for identity checks.
#[must_use]
pub fn decode_signal8(frame: &VideoFrame, color: &InputColor) -> Image {
    let it = InputTransform::new(
        color,
        frame.format.bit_depth(),
        &crate::color::ToneMap::default(),
    );
    let off = frame.chroma_siting.offset();
    let mut img = Image::filled(frame.width, frame.height, [0, 0, 0, 255]);
    for y in 0..frame.height {
        for x in 0..frame.width {
            let (xi, yi) = (i64::from(x), i64::from(y));
            let s = if frame.format.is_yuv() {
                let pos = [(f64::from(x) - off[0]) * 0.5, (f64::from(y) - off[1]) * 0.5];
                let (cb, cr) = match frame.format {
                    PixelFormat::Yuv420p => (
                        plane_bilinear(frame, 1, 0, pos),
                        plane_bilinear(frame, 2, 0, pos),
                    ),
                    _ => (
                        plane_bilinear(frame, 1, 0, pos),
                        plane_bilinear(frame, 1, 1, pos),
                    ),
                };
                it.codes_to_signal(f64::from(frame.code(0, 0, xi, yi)), cb, cr)
            } else {
                let c = |i: usize| f64::from(frame.code(0, i, xi, yi));
                it.codes_to_signal(c(0), c(1), c(2))
            };
            let q = s.map(crate::color::quantize8);
            img.set_pixel(x, y, [q[0], q[1], q[2], 255]);
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use cutweave_model::{
        Color, ColorTags, FrameRate, ItemId, MediaId, SequenceId, Time, TrackId, Vec2,
    };

    use super::*;
    use crate::color::{OutputTransfer, Transfer, from_linear, quantize8};
    use crate::scene::{BlendMode, Crop, Flip, FrameRequest, Layer, LayerTransform};
    use crate::testcard::{Pattern, SyntheticSource, TestCard};

    fn layer(media: u128, size: (u32, u32)) -> Layer {
        Layer {
            item: ItemId::from_u128(1),
            track: TrackId::from_u128(1),
            request: FrameRequest {
                media: MediaId::from_u128(media),
                stream: Some(0),
                source_time: Time::ZERO,
                source_size: Some(size),
                rotation: 0,
                color: ColorTags::default(),
                still: false,
                proxy: false,
                drawn_long_edge: None,
            },
            transform: LayerTransform {
                position: Vec2::ZERO,
                scale: Vec2::ONE,
                rotation_deg: 0.0,
                anchor: Vec2::new(0.5, 0.5),
            },
            opacity: 1.0,
            blend: BlendMode::Normal,
            crop: Crop::default(),
            flip: Flip::default(),
            effects: Vec::new(),
        }
    }

    fn scene(w: u32, h: u32, layers: Vec<Layer>) -> SceneDescription {
        SceneDescription {
            sequence: SequenceId::from_u128(1),
            time: Time::ZERO,
            width: w,
            height: h,
            frame_rate: FrameRate::FPS_24,
            background: Color::BLACK,
            background_node: None,
            layers,
            audio: Vec::new(),
        }
    }

    #[test]
    fn empty_scene_is_the_background() {
        let mut s = scene(8, 4, vec![]);
        s.background = Color::rgb(200, 100, 50);
        let img = render(
            &s,
            Target::rgba8(8, 4),
            &SyntheticSource::new(),
            &GraphOptions::default(),
        )
        .unwrap();
        assert!(
            img.data
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [200, 100, 50, 255])
        );
    }

    #[test]
    fn half_opacity_white_over_black_is_half_linear_light() {
        let white = TestCard::new(
            8,
            8,
            Pattern::Solid([1.0; 3]),
            PixelFormat::Nv12,
            ColorTags::default(),
        );
        let src = SyntheticSource::new().with(MediaId::from_u128(5), white);
        let mut l = layer(5, (8, 8));
        l.opacity = 0.5;
        let img = render(
            &scene(8, 8, vec![l]),
            Target::rgba8(8, 8),
            &src,
            &GraphOptions::default(),
        )
        .unwrap();
        let want = quantize8(from_linear(0.5, Transfer::Srgb));
        assert_eq!(img.pixel(4, 4), [want, want, want, 255]);
    }

    #[test]
    fn identity_reproduces_the_decoded_signal() {
        // Explicit BT.709 tags: untagged SD-sized sources would get BT.601 primaries converted.
        let tags = ColorTags {
            primaries: Some(cutweave_model::ColorPrimaries::Bt709),
            transfer: Some(cutweave_model::TransferCharacteristics::Bt709),
            matrix: Some(cutweave_model::MatrixCoefficients::Bt709),
            range: Some(cutweave_model::ColorRange::Limited),
        };
        let card = TestCard::new(32, 16, Pattern::Sweep, PixelFormat::Yuv420p, tags);
        let frame = card.frame(0);
        let src = SyntheticSource::new().with(MediaId::from_u128(5), card);
        let t = Target::Rgba8Cpu {
            width: 32,
            height: 16,
            transfer: OutputTransfer::Bt709,
        };
        let img = render(
            &scene(32, 16, vec![layer(5, (32, 16))]),
            t,
            &src,
            &GraphOptions::default(),
        )
        .unwrap();
        let color = InputColor::resolve(&frame.color, true, 16);
        let want = decode_signal8(&frame, &color);
        let c = crate::metrics::compare(&img, &want, OutputTransfer::Bt709);
        assert!(c.max_abs_diff <= 1, "{c}");
    }

    #[test]
    fn mips_average_and_trilinear_blends() {
        let base = Level {
            w: 2,
            h: 2,
            px: vec![[0.0; 4], [1.0; 4], [1.0; 4], [0.0; 4]],
        };
        let one = base.downsample();
        assert_eq!((one.w, one.h), (1, 1));
        assert_eq!(one.px[0], [0.5; 4]);
        let levels = vec![base, one];
        // Texel centre (0.5, 0.5) of level 0 is exactly texel 0; LOD 0.5 blends halfway to 0.5.
        assert_eq!(sample(&levels, [0.5, 0.5], 0.0, Filter::Bilinear), [0.0; 4]);
        assert_eq!(
            sample(&levels, [0.5, 0.5], 0.5, Filter::Bilinear),
            [0.25; 4]
        );
        // Beyond the chain, the last level is used.
        assert_eq!(sample(&levels, [0.5, 0.5], 7.3, Filter::Bilinear), [0.5; 4]);
    }

    #[test]
    fn bicubic_reproduces_texels_and_linear_ramps() {
        let ramp = Level {
            w: 8,
            h: 8,
            px: (0..64)
                .map(|i| {
                    let (x, y) = (f64::from(i % 8), f64::from(i / 8));
                    [0.05 * x, 0.04 * y, 0.02 * (x + y), 1.0]
                })
                .collect(),
        };
        let weights = cubic_weights(0.0);
        assert_eq!(weights, [0.0, 1.0, 0.0, 0.0]);
        for f in [0.1, 0.25, 0.5, 0.9] {
            assert!((cubic_weights(f).iter().sum::<f64>() - 1.0).abs() < 1e-15);
        }
        // At texel centres: the texel, exactly.
        assert_eq!(ramp.bicubic([3.5, 4.5]), ramp.px[4 * 8 + 3]);
        // Catmull-Rom reproduces linear functions away from the clamped edges.
        for t in [[3.25, 4.75], [2.6, 3.1], [4.0, 4.0]] {
            let (x, y) = (t[0] - 0.5, t[1] - 0.5);
            let want = [0.05 * x, 0.04 * y, 0.02 * (x + y), 1.0];
            let got = ramp.bicubic(t);
            for c in 0..4 {
                assert!((got[c] - want[c]).abs() < 1e-12, "{t:?}: {got:?}");
            }
        }
        // Ringing at a hard edge is clamped: no negative light, alpha within [0, 1].
        let edge = Level {
            w: 4,
            h: 1,
            px: vec![[0.0, 0.0, 0.0, 0.0], [0.0; 4], [1.0; 4], [1.0; 4]],
        };
        let rung = edge.bicubic([1.75, 0.5]);
        assert!(rung.iter().all(|&c| c >= 0.0) && rung[3] <= 1.0, "{rung:?}");
        let over = edge.bicubic([3.25, 0.5]);
        assert!(over[3] <= 1.0, "{over:?}");
    }

    #[test]
    fn blend_modes_of_solid_layers_are_the_formulas_in_linear_light() {
        // RGBA8 sRGB cards: codes 64 (backdrop) and 192 (layer); linear = sRGB EOTF(code / 255).
        let solid = |v: f64| {
            TestCard::new(
                8,
                8,
                Pattern::Solid([v; 3]),
                PixelFormat::Rgba8,
                ColorTags::default(),
            )
        };
        let src = SyntheticSource::new()
            .with(MediaId::from_u128(1), solid(64.0 / 255.0))
            .with(MediaId::from_u128(2), solid(192.0 / 255.0));
        let lin = |code: f64| crate::color::to_linear(code / 255.0, Transfer::Srgb);
        let (cb, cs) = (lin(64.0), lin(192.0));
        for mode in BlendMode::ALL {
            for opacity in [1.0, 0.4] {
                let mut top = layer(2, (8, 8));
                top.blend = mode;
                top.opacity = opacity;
                let s = scene(8, 8, vec![layer(1, (8, 8)), top]);
                let px = render_linear(&s, (8, 8), &src, &GraphOptions::default()).unwrap();
                let want = cb + opacity * (mode.blend(cb, cs) - cb);
                for p in &px {
                    assert!((p[0] - want).abs() < 1e-12, "{mode:?} {opacity}: {p:?}");
                    assert!((p[3] - 1.0).abs() < 1e-12);
                }
            }
        }
        // Spot values: multiply is the product, screen its complement.
        let m = BlendMode::Multiply.blend(cb, cs);
        assert!((m - cb * cs).abs() < 1e-15);
        let sc = BlendMode::Screen.blend(cb, cs);
        assert!((sc - (1.0 - (1.0 - cb) * (1.0 - cs))).abs() < 1e-15);
    }
}
