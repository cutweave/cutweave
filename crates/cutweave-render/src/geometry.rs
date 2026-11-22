// SPDX-License-Identifier: GPL-3.0-or-later
//! Layer placement: the model's normalized transform units → pixel affines (arch §4.4).
//!
//! [`LayerTransform`](crate::LayerTransform) values arrive from [`evaluate`](crate::evaluate)
//! unchanged from the model (the WP-1F.01 hand-over note on issue WP-1F.02):
//!
//! * `position`: offset of the anchor from the **canvas centre**, in canvas units (1.0 = the
//!   canvas width horizontally, the canvas height vertically; +y down);
//! * `anchor`: the point of the item that `position` places and rotation turns around, in item
//!   units (0..1 of the displayed picture; `(0.5, 0.5)` = its centre);
//! * `scale`: per-axis multiplier **on top of aspect-fit** (1.0 = the picture fitted inside the
//!   canvas, aspect preserved);
//! * `rotation_deg`: clockwise degrees about the anchor.
//!
//! The picture is the source frame **as displayed**: the coded size
//! ([`FrameRequest::source_size`](crate::FrameRequest::source_size)) turned by the media's display
//! rotation ([`FrameRequest::rotation`](crate::FrameRequest::rotation)) before aspect-fit. Layout
//! always uses the probed (original) size, so a proxy or a reduced-size decode samples the same
//! geometry: the decoded frame's own size only enters the display → texel map.
//!
//! The canvas is the sequence frame (`SceneDescription::width × height`); an output of another
//! size (preview quality ladder, thumbnails) scales the whole canvas to it.
//!
//! **Crop** (fractions of the displayed picture removed from each edge) masks the picture without
//! moving it: the visible rectangle shrinks, placement and anchor are unchanged.
//!
//! Coverage uses a ½-output-pixel box approximation at every visible edge (distance to the edge in
//! output pixels + 0.5, clamped to `[0, 1]`), so edges stay one pixel soft at any scale.

// Pixel and matrix math reads best with the conventional single-letter names.
#![allow(clippy::many_single_char_names)]

use crate::scene::Layer;

/// Affine map `[a, b, c, d, e, f]`: `x' = a·x + b·y + c`, `y' = d·x + e·y + f`.
pub type Affine = [f64; 6];

/// The identity map.
pub const AFFINE_IDENTITY: Affine = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

/// Inverts an affine map; `None` if it is singular or not finite.
#[must_use]
pub fn invert_affine(m: &Affine) -> Option<Affine> {
    let [a, b, c, d, e, f] = *m;
    let det = a * e - b * d;
    let norm = a.abs().max(b.abs()).max(d.abs()).max(e.abs());
    if !det.is_finite() || det.abs() <= 1e-12 * norm * norm || norm == 0.0 {
        return None;
    }
    let (ia, ib, id, ie) = (e / det, -b / det, -d / det, a / det);
    let inv = [ia, ib, -(ia * c + ib * f), id, ie, -(id * c + ie * f)];
    inv.iter().all(|v| v.is_finite()).then_some(inv)
}

/// `outer ∘ inner` (apply `inner` first).
#[must_use]
pub fn compose_affine(outer: &Affine, inner: &Affine) -> Affine {
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

/// Applies an affine map to a point.
#[must_use]
pub fn apply_affine(m: &Affine, p: [f64; 2]) -> [f64; 2] {
    [
        m[0] * p[0] + m[1] * p[1] + m[2],
        m[3] * p[0] + m[4] * p[1] + m[5],
    ]
}

/// Normalises a display rotation in degrees to 0, 90, 180 or 270 (nearest quarter turn).
#[must_use]
pub fn quarter_turns(rotation: u16) -> u16 {
    ((u32::from(rotation) % 360 + 45) / 90 % 4) as u16 * 90
}

/// Where one layer's picture lands in an output image, and how output pixels map back to the
/// decoded frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerGeometry {
    /// Size of the displayed picture (coded size after the display rotation), in source pixels.
    pub display_size: [f64; 2],
    /// Display pixels → output pixels.
    pub to_output: Affine,
    /// Output pixels → display pixels.
    pub from_output: Affine,
    /// Display pixels → texel coordinates of the decoded frame (level 0; texel centres at +0.5).
    pub to_texel: Affine,
    /// The visible (cropped) rectangle in display pixels: `[x0, y0, x1, y1]`.
    pub visible: [f64; 4],
    /// Display pixels per output pixel across the x and y edges (`‖∇u‖` of each display axis):
    /// divides a display-space distance to an edge into output pixels.
    pub edge_scale: [f64; 2],
    /// Decoded-frame texels per output pixel (the larger axis): the minification that selects
    /// the mip level.
    pub footprint: f64,
    /// Axis-aligned output-pixel bounds of the visible rectangle `[x0, y0, x1, y1]` (unclipped).
    pub bounds: [f64; 4],
}

impl LayerGeometry {
    /// Computes the placement of `layer` on a `canvas`-sized sequence frame rendered at `output`
    /// size, for a decoded frame of `frame` size. `None` when the layer cannot be drawn: an empty
    /// canvas, output or frame, a non-finite or degenerate (zero-scale) transform, or a crop
    /// that leaves nothing visible.
    #[must_use]
    pub fn new(
        layer: &Layer,
        canvas: (u32, u32),
        output: (u32, u32),
        frame: (u32, u32),
    ) -> Option<LayerGeometry> {
        if [canvas.0, canvas.1, output.0, output.1, frame.0, frame.1].contains(&0) {
            return None;
        }
        let coded = match layer.request.source_size {
            Some((w, h)) if w > 0 && h > 0 => (f64::from(w), f64::from(h)),
            _ => (f64::from(frame.0), f64::from(frame.1)),
        };
        let turns = quarter_turns(layer.request.rotation);
        let display_size = if turns % 180 == 90 {
            [coded.1, coded.0]
        } else {
            [coded.0, coded.1]
        };
        let (cw, ch) = (f64::from(canvas.0), f64::from(canvas.1));
        let t = &layer.transform;

        // Display px → canvas px.
        let fit = (cw / display_size[0]).min(ch / display_size[1]);
        let (kx, ky) = (fit * t.scale.x, fit * t.scale.y);
        let (s, c) = t.rotation_deg.to_radians().sin_cos();
        let (m00, m01, m10, m11) = (c * kx, -s * ky, s * kx, c * ky);
        let (ax, ay) = (t.anchor.x * display_size[0], t.anchor.y * display_size[1]);
        let (px, py) = (cw * (0.5 + t.position.x), ch * (0.5 + t.position.y));
        let to_canvas = [
            m00,
            m01,
            px - m00 * ax - m01 * ay,
            m10,
            m11,
            py - m10 * ax - m11 * ay,
        ];
        // Canvas px → output px.
        let (ox, oy) = (f64::from(output.0) / cw, f64::from(output.1) / ch);
        let to_output = compose_affine(&[ox, 0.0, 0.0, 0.0, oy, 0.0], &to_canvas);
        if !to_output.iter().all(|v| v.is_finite()) {
            return None;
        }
        let from_output = invert_affine(&to_output)?;

        // Display px → coded px (undo the display rotation) → decoded texels.
        let rotate = match turns {
            90 => [0.0, 1.0, 0.0, -1.0, 0.0, coded.1],
            180 => [-1.0, 0.0, coded.0, 0.0, -1.0, coded.1],
            270 => [0.0, -1.0, coded.0, 1.0, 0.0, 0.0],
            _ => AFFINE_IDENTITY,
        };
        let texel_scale = [
            f64::from(frame.0) / coded.0,
            0.0,
            0.0,
            0.0,
            f64::from(frame.1) / coded.1,
            0.0,
        ];
        let to_texel = compose_affine(&texel_scale, &rotate);

        let crop = |v: f64| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let visible = [
            crop(layer.crop.left) * display_size[0],
            crop(layer.crop.top) * display_size[1],
            (1.0 - crop(layer.crop.right)) * display_size[0],
            (1.0 - crop(layer.crop.bottom)) * display_size[1],
        ];
        if visible[2] <= visible[0] || visible[3] <= visible[1] {
            return None;
        }

        let i = &from_output;
        let edge_scale = [i[0].hypot(i[1]), i[3].hypot(i[4])];
        let j = compose_affine(&to_texel, &from_output);
        let footprint = j[0].hypot(j[3]).max(j[1].hypot(j[4]));

        let corners = [
            [visible[0], visible[1]],
            [visible[2], visible[1]],
            [visible[0], visible[3]],
            [visible[2], visible[3]],
        ]
        .map(|p| apply_affine(&to_output, p));
        let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for p in corners {
            bounds = [
                bounds[0].min(p[0]),
                bounds[1].min(p[1]),
                bounds[2].max(p[0]),
                bounds[3].max(p[1]),
            ];
        }
        Some(LayerGeometry {
            display_size,
            to_output,
            from_output,
            to_texel,
            visible,
            edge_scale,
            footprint,
            bounds,
        })
    }

    /// Signed distances (in output pixels) from display point `u` to the four visible edges,
    /// positive inside: `[left, top, right, bottom]`.
    #[must_use]
    pub fn edge_distances(&self, u: [f64; 2]) -> [f64; 4] {
        let [x0, y0, x1, y1] = self.visible;
        [
            (u[0] - x0) / self.edge_scale[0],
            (u[1] - y0) / self.edge_scale[1],
            (x1 - u[0]) / self.edge_scale[0],
            (y1 - u[1]) / self.edge_scale[1],
        ]
    }

    /// Coverage of the output pixel centred at `p` (output pixels), in `[0, 1]`.
    #[must_use]
    pub fn coverage(&self, p: [f64; 2]) -> f64 {
        let d = self.edge_distances(apply_affine(&self.from_output, p));
        let cx = (d[0].min(d[2]) + 0.5).clamp(0.0, 1.0);
        let cy = (d[1].min(d[3]) + 0.5).clamp(0.0, 1.0);
        cx * cy
    }

    /// Whether the layer covers every pixel of an `output`-sized image completely (coverage 1;
    /// an edge exactly on the output border counts). The compositor then skips the coverage
    /// computation (exactly 1, immune to `f32` rounding), and with opaque content at opacity 1
    /// and normal blend the layer hides everything below it exactly.
    #[must_use]
    pub fn covers(&self, output: (u32, u32)) -> bool {
        const MARGIN: f64 = 0.5 - 1e-9;
        let (w, h) = (f64::from(output.0), f64::from(output.1));
        // Coverage is the minimum of affine functions, so checking the corner pixel centres of
        // the (convex) output rectangle suffices.
        [
            [0.5, 0.5],
            [w - 0.5, 0.5],
            [0.5, h - 0.5],
            [w - 0.5, h - 0.5],
        ]
        .iter()
        .all(|&p| {
            self.edge_distances(apply_affine(&self.from_output, p))
                .iter()
                .all(|&d| d >= MARGIN)
        })
    }

    /// Whether no output pixel of a `output`-sized image gets any coverage.
    #[must_use]
    pub fn offscreen(&self, output: (u32, u32)) -> bool {
        let [x0, y0, x1, y1] = self.bounds;
        x1 <= -0.5
            || y1 <= -0.5
            || x0 >= f64::from(output.0) + 0.5
            || y0 >= f64::from(output.1) + 0.5
    }

    /// The mip level of detail: `log2(footprint)` when minifying, else 0.
    #[must_use]
    pub fn lod(&self) -> f64 {
        if self.footprint > 1.0 {
            self.footprint.log2()
        } else {
            0.0
        }
    }

    /// The axis-aligned output-pixel rectangle (integer, clipped to the output) that contains
    /// every pixel with non-zero coverage: `[x0, y0, x1, y1]`, `x1`/`y1` exclusive.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn pixel_rect(&self, output: (u32, u32)) -> [u32; 4] {
        let clip = |v: f64, max: u32| v.clamp(0.0, f64::from(max)) as u32;
        [
            clip((self.bounds[0] - 1.0).floor(), output.0),
            clip((self.bounds[1] - 1.0).floor(), output.1),
            clip((self.bounds[2] + 1.0).ceil(), output.0),
            clip((self.bounds[3] + 1.0).ceil(), output.1),
        ]
    }
}

/// Mip levels of a `w × h` texture down to 1×1 (`floor(log2(max)) + 1`).
#[must_use]
pub fn full_mip_count(w: u32, h: u32) -> u32 {
    32 - w.max(h).max(1).leading_zeros()
}

/// Mip levels a layer sampled at `lod` needs: 1 when not minifying, else enough for trilinear
/// filtering between `floor(lod)` and the next level, capped at the full chain.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn mip_levels_for(lod: f64, w: u32, h: u32) -> u32 {
    if lod <= 0.0 || !lod.is_finite() {
        return 1;
    }
    (lod.floor().min(30.0) as u32 + 2).min(full_mip_count(w, h))
}

/// Size of mip `level` of a `w × h` texture.
#[must_use]
pub fn mip_size(w: u32, h: u32, level: u32) -> (u32, u32) {
    ((w >> level).max(1), (h >> level).max(1))
}

#[cfg(test)]
mod tests {
    use cutweave_model::{ColorTags, ItemId, MediaId, Time, TrackId, Vec2};
    use proptest::prelude::*;

    use super::*;
    use crate::scene::{BlendMode, Crop, FrameRequest, LayerTransform};

    fn layer(source: (u32, u32), rotation: u16, transform: LayerTransform) -> Layer {
        Layer {
            item: ItemId::from_u128(1),
            track: TrackId::from_u128(2),
            request: FrameRequest {
                media: MediaId::from_u128(3),
                stream: Some(0),
                source_time: Time::ZERO,
                source_size: Some(source),
                rotation,
                color: ColorTags::default(),
                still: false,
            },
            transform,
            opacity: 1.0,
            blend: BlendMode::Normal,
            crop: Crop::default(),
            effects: Vec::new(),
        }
    }

    fn identity() -> LayerTransform {
        LayerTransform {
            position: Vec2::ZERO,
            scale: Vec2::ONE,
            rotation_deg: 0.0,
            anchor: Vec2::new(0.5, 0.5),
        }
    }

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    fn geo(l: &Layer, canvas: (u32, u32), output: (u32, u32)) -> LayerGeometry {
        let frame = l.request.source_size.unwrap_or((16, 16));
        LayerGeometry::new(l, canvas, output, frame).expect("drawable")
    }

    #[test]
    fn identity_maps_source_pixels_to_canvas_pixels() {
        let g = geo(
            &layer((1920, 1080), 0, identity()),
            (1920, 1080),
            (1920, 1080),
        );
        for (i, v) in g.to_output.iter().enumerate() {
            assert!((v - AFFINE_IDENTITY[i]).abs() < 1e-12, "{:?}", g.to_output);
        }
        assert!(close(apply_affine(&g.to_texel, [3.5, 7.5]), [3.5, 7.5]));
        assert!((g.footprint - 1.0).abs() < 1e-12 && g.lod() == 0.0);
        assert!(g.covers((1920, 1080)));
        assert!((g.coverage([0.5, 0.5]) - 1.0).abs() < 1e-12);
        assert_eq!(g.pixel_rect((1920, 1080)), [0, 0, 1920, 1080]);
    }

    #[test]
    fn aspect_fit_pillarboxes_and_letterboxes() {
        // Square into 16:9: 1080 px wide, centred → x from 420 to 1500.
        let g = geo(
            &layer((1080, 1080), 0, identity()),
            (1920, 1080),
            (1920, 1080),
        );
        assert!(close(apply_affine(&g.to_output, [0.0, 0.0]), [420.0, 0.0]));
        assert!(close(
            apply_affine(&g.to_output, [1080.0, 1080.0]),
            [1500.0, 1080.0]
        ));
        assert!(!g.covers((1920, 1080)));
        // 2.4:1 into 16:9: full width, 800 px tall → y from 140 to 940.
        let g = geo(
            &layer((1920, 800), 0, identity()),
            (1920, 1080),
            (1920, 1080),
        );
        assert!(close(apply_affine(&g.to_output, [0.0, 0.0]), [0.0, 140.0]));
        assert!(close(
            apply_affine(&g.to_output, [1920.0, 800.0]),
            [1920.0, 940.0]
        ));
        // A small source is scaled up to fit (4×), so it minifies nothing.
        let g = geo(
            &layer((480, 270), 0, identity()),
            (1920, 1080),
            (1920, 1080),
        );
        assert!(close(
            apply_affine(&g.to_output, [480.0, 270.0]),
            [1920.0, 1080.0]
        ));
        assert!((g.footprint - 0.25).abs() < 1e-12);
    }

    #[test]
    fn position_is_in_canvas_units_from_the_centre() {
        let mut t = identity();
        t.position = Vec2::new(0.25, -0.5);
        let g = geo(&layer((1920, 1080), 0, t), (1920, 1080), (1920, 1080));
        // The anchor (picture centre) lands at centre + (0.25·1920, −0.5·1080).
        assert!(close(
            apply_affine(&g.to_output, [960.0, 540.0]),
            [1440.0, 0.0]
        ));
    }

    #[test]
    fn scale_multiplies_aspect_fit_about_the_anchor() {
        // Half size, anchored at the top-left corner, placed at the canvas top-left.
        let t = LayerTransform {
            position: Vec2::new(-0.5, -0.5),
            scale: Vec2::new(0.5, 0.5),
            rotation_deg: 0.0,
            anchor: Vec2::ZERO,
        };
        let g = geo(&layer((3840, 2160), 0, t), (1920, 1080), (1920, 1080));
        assert!(close(apply_affine(&g.to_output, [0.0, 0.0]), [0.0, 0.0]));
        assert!(close(
            apply_affine(&g.to_output, [3840.0, 2160.0]),
            [960.0, 540.0]
        ));
        // 4K at half of fit → 4 texels per output pixel → LOD 2.
        assert!((g.footprint - 4.0).abs() < 1e-9 && (g.lod() - 2.0).abs() < 1e-9);
        assert_eq!(mip_levels_for(g.lod(), 3840, 2160), 4);
    }

    #[test]
    fn rotation_is_clockwise_about_the_anchor() {
        let mut t = identity();
        t.rotation_deg = 90.0;
        let g = geo(&layer((1920, 1080), 0, t), (1920, 1080), (1920, 1080));
        // The centre stays put; the right-edge midpoint swings down (y-down, clockwise).
        assert!(close(
            apply_affine(&g.to_output, [960.0, 540.0]),
            [960.0, 540.0]
        ));
        assert!(close(
            apply_affine(&g.to_output, [1920.0, 540.0]),
            [960.0, 1500.0]
        ));
        assert!(close(
            apply_affine(&g.to_output, [960.0, 0.0]),
            [1500.0, 540.0]
        ));
    }

    #[test]
    fn media_display_rotation_is_applied_before_aspect_fit() {
        // A portrait phone clip: coded 1920×1080 with a 90° display rotation shows 1080×1920,
        // fitted into a 1920×1080 canvas as 607.5 × 1080, centred.
        let g = geo(
            &layer((1920, 1080), 90, identity()),
            (1920, 1080),
            (1920, 1080),
        );
        assert_eq!(g.display_size, [1080.0, 1920.0]);
        let half = 607.5 / 2.0;
        assert!(close(
            apply_affine(&g.to_output, [0.0, 0.0]),
            [960.0 - half, 0.0]
        ));
        // The coded top-left pixel appears at the displayed top-right corner.
        assert!(close(apply_affine(&g.to_texel, [1080.0, 0.0]), [0.0, 0.0]));
        assert!(close(
            apply_affine(&g.to_texel, [1080.0, 1920.0]),
            [1920.0, 0.0]
        ));
        for (rot, display_corner) in [(180, [1920.0, 1080.0]), (270, [0.0, 1920.0])] {
            let g = geo(
                &layer((1920, 1080), rot, identity()),
                (1920, 1080),
                (1920, 1080),
            );
            assert!(
                close(apply_affine(&g.to_texel, display_corner), [0.0, 0.0]),
                "{rot}°: {:?}",
                apply_affine(&g.to_texel, display_corner)
            );
        }
        // Arbitrary angles snap to the nearest quarter turn.
        assert_eq!(quarter_turns(89), 90);
        assert_eq!(quarter_turns(359), 0);
        assert_eq!(quarter_turns(450), 90);
    }

    #[test]
    fn output_size_scales_the_whole_canvas() {
        let full = geo(
            &layer((1920, 1080), 0, identity()),
            (1920, 1080),
            (1920, 1080),
        );
        let half = geo(
            &layer((1920, 1080), 0, identity()),
            (1920, 1080),
            (960, 540),
        );
        let p = apply_affine(&full.to_output, [100.0, 50.0]);
        let q = apply_affine(&half.to_output, [100.0, 50.0]);
        assert!(close([p[0] / 2.0, p[1] / 2.0], q));
        assert!((half.footprint - 2.0).abs() < 1e-12 && (half.lod() - 1.0).abs() < 1e-12);
        assert!((half.edge_scale[0] - 2.0).abs() < 1e-12);
    }

    #[test]
    fn proxies_keep_the_geometry_and_rescale_texels() {
        // A half-resolution decode of a 1920×1080 source lands in the same place.
        let l = layer((1920, 1080), 0, identity());
        let full = LayerGeometry::new(&l, (1920, 1080), (1920, 1080), (1920, 1080)).unwrap();
        let proxy = LayerGeometry::new(&l, (1920, 1080), (1920, 1080), (960, 540)).unwrap();
        assert_eq!(full.to_output, proxy.to_output);
        assert!(close(
            apply_affine(&proxy.to_texel, [1920.0, 1080.0]),
            [960.0, 540.0]
        ));
        // Without a probed size the decoded size is the layout size.
        let mut unprobed = l.clone();
        unprobed.request.source_size = None;
        let g = LayerGeometry::new(&unprobed, (1920, 1080), (1920, 1080), (960, 540)).unwrap();
        assert_eq!(g.display_size, [960.0, 540.0]);
    }

    #[test]
    fn crop_masks_without_moving_the_picture() {
        let mut l = layer((1920, 1080), 0, identity());
        l.crop = Crop {
            left: 0.25,
            top: 0.0,
            right: 0.0,
            bottom: 0.5,
        };
        let g = geo(&l, (1920, 1080), (1920, 1080));
        assert_eq!(g.visible, [480.0, 0.0, 1920.0, 540.0]);
        assert_eq!(
            g.to_output,
            geo(
                &layer((1920, 1080), 0, identity()),
                (1920, 1080),
                (1920, 1080)
            )
            .to_output
        );
        assert_eq!(g.coverage([100.5, 100.5]), 0.0);
        assert_eq!(g.coverage([1000.5, 100.5]), 1.0);
        // Half a pixel either side of the crop edge.
        assert!((g.coverage([480.0, 100.5]) - 0.5).abs() < 1e-12);
        assert!(!g.covers((1920, 1080)));
        l.crop.left = 0.6;
        l.crop.right = 0.4;
        assert!(LayerGeometry::new(&l, (1920, 1080), (1920, 1080), (1920, 1080)).is_none());
    }

    #[test]
    fn edges_are_half_an_output_pixel_soft_at_any_scale() {
        let mut t = identity();
        t.scale = Vec2::new(0.25, 0.25);
        let g = geo(&layer((1920, 1080), 0, t), (1920, 1080), (1920, 1080));
        // The layer spans x 720..1200; a pixel centred on the edge is half covered.
        let left = apply_affine(&g.to_output, [0.0, 540.0])[0];
        assert!((left - 720.0).abs() < 1e-9);
        assert!((g.coverage([720.0, 540.5]) - 0.5).abs() < 1e-9);
        assert!((g.coverage([719.0, 540.5])).abs() < 1e-9);
        assert!((g.coverage([721.0, 540.5]) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_and_offscreen_layers() {
        let mut t = identity();
        t.scale = Vec2::new(0.0, 1.0);
        assert!(LayerGeometry::new(&layer((64, 64), 0, t), (64, 64), (64, 64), (64, 64)).is_none());
        t.scale = Vec2::new(f64::NAN, 1.0);
        assert!(LayerGeometry::new(&layer((64, 64), 0, t), (64, 64), (64, 64), (64, 64)).is_none());
        assert!(
            LayerGeometry::new(&layer((64, 64), 0, identity()), (0, 64), (64, 64), (64, 64))
                .is_none()
        );
        let mut t = identity();
        t.position = Vec2::new(2.0, 0.0);
        let g = geo(&layer((64, 64), 0, t), (64, 64), (64, 64));
        assert!(g.offscreen((64, 64)));
        t.position = Vec2::new(1.0 - 0.1, 0.0);
        assert!(!geo(&layer((64, 64), 0, t), (64, 64), (64, 64)).offscreen((64, 64)));
    }

    #[test]
    fn mip_chain_sizes() {
        assert_eq!(full_mip_count(1920, 1080), 11);
        assert_eq!(full_mip_count(1, 1), 1);
        assert_eq!(mip_size(1920, 1080, 1), (960, 540));
        assert_eq!(mip_size(5, 3, 2), (1, 1));
        assert_eq!(mip_levels_for(0.0, 1920, 1080), 1);
        assert_eq!(mip_levels_for(0.4, 1920, 1080), 2);
        assert_eq!(mip_levels_for(40.0, 4, 4), 3);
    }

    proptest! {
        #[test]
        fn forward_and_inverse_agree(
            px in -1.0f64..1.0, py in -1.0f64..1.0,
            sx in 0.05f64..4.0, sy in 0.05f64..4.0,
            rot in -720.0f64..720.0,
            ax in 0.0f64..1.0, ay in 0.0f64..1.0,
            w in 16u32..4000, h in 16u32..4000, turns in 0u16..4,
            x in 0.0f64..1.0, y in 0.0f64..1.0,
        ) {
            let t = LayerTransform {
                position: Vec2::new(px, py),
                scale: Vec2::new(sx, sy),
                rotation_deg: rot,
                anchor: Vec2::new(ax, ay),
            };
            let l = layer((w, h), turns * 90, t);
            let g = LayerGeometry::new(&l, (1920, 1080), (1280, 720), (w, h)).unwrap();
            let p = [x * g.display_size[0], y * g.display_size[1]];
            let back = apply_affine(&g.from_output, apply_affine(&g.to_output, p));
            prop_assert!((back[0] - p[0]).abs() < 1e-6 * g.display_size[0].max(1.0));
            prop_assert!((back[1] - p[1]).abs() < 1e-6 * g.display_size[1].max(1.0));
            // The anchor always lands at the positioned canvas point (scaled to the output).
            let a = apply_affine(&g.to_output, [ax * g.display_size[0], ay * g.display_size[1]]);
            prop_assert!((a[0] - 1280.0 * (0.5 + px)).abs() < 1e-6);
            prop_assert!((a[1] - 720.0 * (0.5 + py)).abs() < 1e-6);
            // Every texel of the displayed picture maps inside the decoded frame.
            let tx = apply_affine(&g.to_texel, p);
            prop_assert!(tx[0] >= -1e-6 && tx[0] <= f64::from(w) + 1e-6);
            prop_assert!(tx[1] >= -1e-6 && tx[1] <= f64::from(h) + 1e-6);
        }
    }
}
