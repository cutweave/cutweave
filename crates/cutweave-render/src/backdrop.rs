// SPDX-License-Identifier: GPL-3.0-or-later
//! The background fill of a sequence (WP-2B.02; arch §8): what is drawn behind all layers, over
//! the background colour. The rules here are shared by the graph builder (GPU) and the CPU
//! reference, which implement the passes independently.
//!
//! * **Colour**: the canvas starts as the colour (a clear).
//! * **Transparent**: the canvas starts as transparent black; the colour is not drawn.
//! * **Image**: the still is placed over the colour as a bottom layer (a textured quad), sized
//!   by its fit ([`image_layer`]): `cover` fills the canvas and crops, `contain` shows the whole
//!   image, `stretch` gives it the canvas's shape.
//! * **Blur**: a copy of one layer's picture (its crop and flip kept, its transform, opacity and
//!   blend mode ignored) covers the canvas ([`cover_layer`]) and is blurred:
//!   1. *downsample*: the picture is drawn, over the colour, onto a small canvas
//!      ([`BlurPlan::size`]: the output divided by a power of two) with the compositor's own
//!      trilinear minification;
//!   2. *blur*: a separable Gaussian, one horizontal and one vertical pass on the small canvas
//!      (weights `exp(−i² / 2σ²)` for `i` in `−taps..=taps`, normalised, edges clamped); the
//!      vertical pass also multiplies the colour by `1 − dim`;
//!   3. *upsample*: the result is stretched over the whole canvas with exact-weight bilinear
//!      filtering.
//!
//!   The radius is a fraction of the canvas height, so the blur looks the same at every output
//!   size: a half-size preview halves the divisor and keeps the small canvas and its `σ`
//!   ([`blur_plan`]).
//! * **Dither**: a frame with a blurred background is quantised to 8 bits with an ordered
//!   dither ([`dither_threshold`]) instead of rounding half up, so its smooth gradients do not
//!   band. Other frames keep the plain rounding (their pixels do not change).
//!
//! All of it is in premultiplied linear light, like every other pass.

// Pixel math reads best with the conventional single-letter names.
#![allow(clippy::many_single_char_names)]

use cutweave_model::{BackgroundFit, ItemId, TrackId, Vec2};

use crate::geometry::{AFFINE_IDENTITY, LayerGeometry, quarter_turns};
use crate::scene::{BlendMode, Crop, Flip, FrameRequest, Layer, LayerTransform};

/// The smallest `σ`, in texels of the small canvas, the blur keeps before it halves the canvas
/// again: the small canvas is the largest power-of-two division of the output whose `σ` is
/// still at least this, so `σ` stays in `[2, 4)` and the kernel within [`MAX_TAPS`].
pub const MIN_SMALL_SIGMA: f64 = 2.0;

/// The most taps on each side of the centre a blur pass reads (`⌈3σ⌉` for `σ < 4`, with room
/// for the rounding of the small canvas's size).
pub const MAX_TAPS: u32 = 16;

/// The most halvings of the output for the small canvas.
const MAX_SHIFT: u32 = 12;

/// How a blurred background is computed for one output size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurPlan {
    /// Size of the small canvas the blur runs on.
    pub size: (u32, u32),
    /// The Gaussian's standard deviation in small-canvas texels, per axis (x, y).
    pub sigma: [f64; 2],
    /// Taps on each side of the centre, per axis: `⌈3σ⌉`, at most [`MAX_TAPS`]; 0 = no blur.
    pub taps: [u32; 2],
}

/// The plan for a blur of `radius` (a fraction of the canvas height, clamped to `0..=1`) on an
/// `output`-sized frame.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn blur_plan(radius: f64, output: (u32, u32)) -> BlurPlan {
    let radius = if radius.is_finite() {
        radius.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let sigma_out = radius * f64::from(output.1);
    let mut shift = 0;
    while shift < MAX_SHIFT && sigma_out / f64::from(1_u32 << (shift + 1)) >= MIN_SMALL_SIGMA {
        shift += 1;
    }
    let d = 1_u32 << shift;
    let size = (output.0.div_ceil(d).max(1), output.1.div_ceil(d).max(1));
    let sigma = [
        sigma_out * f64::from(size.0) / f64::from(output.0.max(1)),
        sigma_out * f64::from(size.1) / f64::from(output.1.max(1)),
    ];
    let taps = sigma.map(|s| {
        if s <= 1e-6 {
            0
        } else {
            ((3.0 * s).ceil() as u32).clamp(1, MAX_TAPS)
        }
    });
    BlurPlan { size, sigma, taps }
}

/// The unnormalised Gaussian weight of the texel `i` away from the centre.
#[must_use]
pub fn gaussian_weight(i: i32, sigma: f64) -> f64 {
    let x = f64::from(i);
    (-(x * x) / (2.0 * sigma * sigma)).exp()
}

/// The ordered-dither threshold of output pixel `(x, y)`, in `(0, 1)`: the 8×8 Bayer matrix,
/// `(m + ½) / 64`. A value `v` in `[0, 1]` quantises to `⌊255·v + threshold⌋`; plain rounding
/// is the same with a threshold of ½.
#[must_use]
pub fn dither_threshold(x: u32, y: u32) -> f64 {
    let mut m = 0_u32;
    for bit in 0..3 {
        let (xb, yb) = ((x >> bit) & 1, (y >> bit) & 1);
        m = (m << 2) | ((xb ^ yb) << 1) | yb;
    }
    (f64::from(m) + 0.5) / 64.0
}

/// The upright picture's size for layout: the probed coded size (else the decoded frame's)
/// turned by the display rotation.
fn display_size(request: &FrameRequest, frame: (u32, u32)) -> (f64, f64) {
    let (w, h) = match request.source_size {
        Some((w, h)) if w > 0 && h > 0 => (f64::from(w), f64::from(h)),
        _ => (f64::from(frame.0), f64::from(frame.1)),
    };
    if quarter_turns(request.rotation) % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    }
}

/// A synthetic bottom layer showing `request` with `transform`.
fn fill_layer(request: &FrameRequest, crop: Crop, flip: Flip, transform: LayerTransform) -> Layer {
    Layer {
        item: ItemId::from_u128(0),
        track: TrackId::from_u128(0),
        request: request.clone(),
        transform,
        opacity: 1.0,
        blend: BlendMode::Normal,
        crop,
        flip,
        effects: Vec::new(),
    }
}

/// The layer a blurred background draws: `source`'s picture, crop and flip kept, scaled so
/// that what the crop leaves covers a `canvas`-sized frame, centred. `frame` is the decoded
/// frame's size (used when the source size was not probed).
#[must_use]
pub fn cover_layer(source: &Layer, frame: (u32, u32), canvas: (u32, u32)) -> Layer {
    let (dw, dh) = display_size(&source.request, frame);
    let edge = |v: f64| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let c = &source.crop;
    let (mut left, mut right) = (edge(c.left), edge(c.right));
    let (mut top, mut bottom) = (edge(c.top), edge(c.bottom));
    // Item space is the upright picture mirrored by the flip.
    if source.flip.horizontal {
        std::mem::swap(&mut left, &mut right);
    }
    if source.flip.vertical {
        std::mem::swap(&mut top, &mut bottom);
    }
    let (vw, vh) = ((1.0 - left - right) * dw, (1.0 - top - bottom) * dh);
    let (cw, ch) = (f64::from(canvas.0), f64::from(canvas.1));
    let fit = (cw / dw).min(ch / dh);
    let scale = if vw > 0.0 && vh > 0.0 && fit > 0.0 {
        (cw / vw).max(ch / vh) / fit
    } else {
        1.0
    };
    fill_layer(
        &source.request,
        source.crop,
        source.flip,
        LayerTransform {
            position: Vec2::ZERO,
            scale: Vec2::new(scale, scale),
            rotation_deg: 0.0,
            // The centre of what the crop leaves sits on the canvas centre.
            anchor: Vec2::new((left + (1.0 - right)) * 0.5, (top + (1.0 - bottom)) * 0.5),
        },
    )
}

/// The layer an image background draws: the still of `request` sized to a `canvas`-sized frame
/// by `fit`, centred. `frame` is the decoded frame's size.
#[must_use]
pub fn image_layer(
    request: &FrameRequest,
    fit: BackgroundFit,
    frame: (u32, u32),
    canvas: (u32, u32),
) -> Layer {
    let (dw, dh) = display_size(request, frame);
    let (cw, ch) = (f64::from(canvas.0), f64::from(canvas.1));
    let (kx, ky) = (cw / dw, ch / dh);
    let contain = kx.min(ky);
    let scale = if contain > 0.0 && contain.is_finite() {
        match fit {
            BackgroundFit::Contain => Vec2::ONE,
            BackgroundFit::Cover => {
                let s = kx.max(ky) / contain;
                Vec2::new(s, s)
            }
            BackgroundFit::Stretch => Vec2::new(kx / contain, ky / contain),
        }
    } else {
        Vec2::ONE
    };
    fill_layer(
        request,
        Crop::default(),
        Flip::default(),
        LayerTransform {
            position: Vec2::ZERO,
            scale,
            rotation_deg: 0.0,
            anchor: Vec2::new(0.5, 0.5),
        },
    )
}

/// The placement of a `texture`-sized picture stretched over the whole of an `output`-sized
/// frame (the blur's upsample): output pixel centre `p` reads texel `p · texture / output`.
#[must_use]
pub fn stretch_geometry(texture: (u32, u32), output: (u32, u32)) -> LayerGeometry {
    let (tw, th) = (f64::from(texture.0), f64::from(texture.1));
    let (ow, oh) = (f64::from(output.0), f64::from(output.1));
    let (sx, sy) = (tw / ow, th / oh);
    LayerGeometry {
        display_size: [tw, th],
        to_output: [1.0 / sx, 0.0, 0.0, 0.0, 1.0 / sy, 0.0],
        from_output: [sx, 0.0, 0.0, 0.0, sy, 0.0],
        to_texel: AFFINE_IDENTITY,
        visible: [0.0, 0.0, tw, th],
        edge_scale: [sx, sy],
        footprint: sx.max(sy),
        bounds: [0.0, 0.0, ow, oh],
    }
}

#[cfg(test)]
mod tests {
    use cutweave_model::{ColorTags, MediaId, Time};

    use super::*;
    use crate::geometry::apply_affine;

    fn request(size: (u32, u32), rotation: u16) -> FrameRequest {
        FrameRequest {
            media: MediaId::from_u128(1),
            stream: Some(0),
            source_time: Time::ZERO,
            source_size: Some(size),
            rotation,
            color: ColorTags::default(),
            still: true,
            proxy: false,
            drawn_long_edge: None,
        }
    }

    fn source(size: (u32, u32), crop: Crop, flip: Flip) -> Layer {
        fill_layer(
            &request(size, 0),
            crop,
            flip,
            LayerTransform {
                position: Vec2::new(0.3, -0.2),
                scale: Vec2::new(0.25, 0.25),
                rotation_deg: 33.0,
                anchor: Vec2::new(0.1, 0.9),
            },
        )
    }

    /// The output-pixel rectangle the layer's visible part lands on.
    fn placed(layer: &Layer, canvas: (u32, u32), output: (u32, u32)) -> [f64; 4] {
        let size = layer.request.source_size.unwrap_or(canvas);
        LayerGeometry::new(layer, canvas, output, size)
            .expect("drawable")
            .bounds
    }

    fn near(a: [f64; 4], b: [f64; 4]) {
        for i in 0..4 {
            assert!((a[i] - b[i]).abs() < 1e-6, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn the_radius_is_relative_to_the_height_at_every_preview_scale() {
        // 5% of 1920 px is σ = 96 px: five halvings leave σ = 3 on a 34 × 60 canvas.
        let full = blur_plan(0.05, (1080, 1920));
        assert_eq!(full.size, (34, 60));
        assert!((full.sigma[1] - 3.0).abs() < 1e-12, "{full:?}");
        assert_eq!(full.taps, [10, 9]);
        // Half and quarter previews blur on the same small canvas with the same σ.
        for output in [(540, 960), (270, 480)] {
            let plan = blur_plan(0.05, output);
            assert_eq!(plan.size, full.size, "{output:?}");
            assert!((plan.sigma[1] - full.sigma[1]).abs() < 1e-12);
            assert!((plan.sigma[0] - full.sigma[0]).abs() < 1e-12);
            assert_eq!(plan.taps, full.taps);
        }
        // σ in output pixels follows the output height.
        let half = blur_plan(0.05, (540, 960));
        let per_output = |p: BlurPlan, h: u32| p.sigma[1] * f64::from(h) / f64::from(p.size.1);
        assert!((per_output(full, 1920) - 2.0 * per_output(half, 960)).abs() < 1e-9);
    }

    #[test]
    fn plans_stay_within_the_kernel_at_any_radius_and_size() {
        for output in [(1, 1), (7, 3), (640, 360), (1080, 1920), (16384, 16384)] {
            for step in 0..=200 {
                let plan = blur_plan(f64::from(step) / 200.0, output);
                assert!(plan.size.0 >= 1 && plan.size.1 >= 1);
                assert!(plan.size.0 <= output.0 && plan.size.1 <= output.1);
                for axis in 0..2 {
                    assert!(plan.taps[axis] <= MAX_TAPS, "{plan:?}");
                    // The kernel reaches 3σ unless the canvas could not be halved further.
                    assert!(
                        f64::from(plan.taps[axis]) >= (3.0 * plan.sigma[axis]).min(12.0) - 1e-9
                            || plan.taps[axis] == MAX_TAPS,
                        "{plan:?}"
                    );
                }
            }
        }
        assert_eq!(blur_plan(0.0, (640, 360)).taps, [0, 0]);
        assert_eq!(blur_plan(f64::NAN, (640, 360)).taps, [0, 0]);
        assert_eq!(blur_plan(0.0, (640, 360)).size, (640, 360));
    }

    #[test]
    fn the_dither_matrix_is_bayer_8() {
        // The 2 × 2 corner of the classic matrix, scaled to 64 levels.
        let m = |x, y| (dither_threshold(x, y) * 64.0 - 0.5).round() as u32;
        assert_eq!([m(0, 0), m(1, 0), m(0, 1), m(1, 1)], [0, 32, 48, 16]);
        // Every level once per 8 × 8 tile, repeating.
        let mut seen = [false; 64];
        for y in 0..8 {
            for x in 0..8 {
                seen[m(x, y) as usize] = true;
                assert_eq!(m(x, y), m(x + 8, y + 16));
            }
        }
        assert!(seen.iter().all(|s| *s));
        // The thresholds average ½, so the dither keeps the mean.
        let mean: f64 = (0..64).map(|i| dither_threshold(i % 8, i / 8)).sum::<f64>() / 64.0;
        assert!((mean - 0.5).abs() < 1e-12);
    }

    #[test]
    fn a_blur_covers_the_canvas_whatever_the_clip_transform() {
        // 16:9 on 9:16: the height fits, the width overflows equally on both sides.
        let l = cover_layer(
            &source((1920, 1080), Crop::default(), Flip::default()),
            (1920, 1080),
            (1080, 1920),
        );
        assert_eq!(l.opacity, 1.0);
        assert_eq!(l.blend, BlendMode::Normal);
        let w = 1920.0 * 1920.0 / 1080.0;
        near(
            placed(&l, (1080, 1920), (1080, 1920)),
            [(1080.0 - w) / 2.0, 0.0, (1080.0 + w) / 2.0, 1920.0],
        );
        // On the small canvas the same picture covers the same share.
        near(
            placed(&l, (1080, 1920), (34, 60)),
            [
                (1080.0 - w) / 2.0 * 34.0 / 1080.0,
                0.0,
                (1080.0 + w) / 2.0 * 34.0 / 1080.0,
                60.0,
            ],
        );
    }

    #[test]
    fn a_cropped_and_flipped_source_covers_with_what_the_crop_leaves() {
        let crop = Crop {
            left: 0.5,
            top: 0.0,
            right: 0.0,
            bottom: 0.25,
        };
        for flip in [
            Flip::default(),
            Flip {
                horizontal: true,
                vertical: true,
            },
        ] {
            // What is left is 960 × 810 (32:27) on a square canvas: the height fits.
            let l = cover_layer(
                &source((1920, 1080), crop, flip),
                (1920, 1080),
                (1000, 1000),
            );
            let w = 1000.0 * 960.0 / 810.0;
            near(
                placed(&l, (1000, 1000), (1000, 1000)),
                [(1000.0 - w) / 2.0, 0.0, (1000.0 + w) / 2.0, 1000.0],
            );
            assert_eq!((l.crop, l.flip), (crop, flip));
        }
    }

    #[test]
    fn image_fits() {
        let canvas = (1080, 1920);
        let r = request((1000, 500), 0);
        near(
            placed(
                &image_layer(&r, BackgroundFit::Contain, (1000, 500), canvas),
                canvas,
                canvas,
            ),
            [0.0, 690.0, 1080.0, 1230.0],
        );
        near(
            placed(
                &image_layer(&r, BackgroundFit::Cover, (1000, 500), canvas),
                canvas,
                canvas,
            ),
            [-1380.0, 0.0, 2460.0, 1920.0],
        );
        near(
            placed(
                &image_layer(&r, BackgroundFit::Stretch, (1000, 500), canvas),
                canvas,
                canvas,
            ),
            [0.0, 0.0, 1080.0, 1920.0],
        );
        // A rotated still is laid out upright; an unprobed one by its decoded size.
        let turned = request((500, 1000), 90);
        near(
            placed(
                &image_layer(&turned, BackgroundFit::Contain, (500, 1000), canvas),
                canvas,
                canvas,
            ),
            [0.0, 690.0, 1080.0, 1230.0],
        );
        let mut unprobed = request((1, 1), 0);
        unprobed.source_size = None;
        let l = image_layer(&unprobed, BackgroundFit::Stretch, (1000, 500), canvas);
        assert!((l.transform.scale.y / l.transform.scale.x - 1920.0 / 540.0).abs() < 1e-9);
    }

    #[test]
    fn the_stretch_maps_the_whole_texture_onto_the_whole_output() {
        let g = stretch_geometry((34, 60), (1080, 1920));
        assert!(g.covers((1080, 1920)));
        assert_eq!(g.lod(), 0.0);
        assert_eq!(g.pixel_rect((1080, 1920)), [0, 0, 1080, 1920]);
        let t = apply_affine(&g.to_texel, apply_affine(&g.from_output, [540.0, 960.0]));
        assert!((t[0] - 17.0).abs() < 1e-9 && (t[1] - 30.0).abs() < 1e-9);
        let back = apply_affine(&g.to_output, [34.0, 60.0]);
        assert!((back[0] - 1080.0).abs() < 1e-9 && (back[1] - 1920.0).abs() < 1e-9);
    }
}
