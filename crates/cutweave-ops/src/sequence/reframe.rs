// SPDX-License-Identifier: GPL-3.0-or-later
//! Reframing clip transforms when a sequence's canvas changes size or shape (WP-2B.01).
//!
//! Transforms are resolution-independent (arch §4.2): `position` is in canvas units (1.0 is
//! the canvas width across and the canvas height down) and `scale` multiplies the aspect-fit
//! size of the picture. A new canvas therefore changes where a clip lands and how big it is, even
//! though its numbers stay the same. The policy decides how the numbers change.
//!
//! # Algorithm
//!
//! Work in canvas pixels. The old canvas is `Wo x Ho`, the new one `Wn x Hn`. A policy picks one
//! uniform factor `k` for the whole picture:
//!
//! | policy | `k` | result |
//! |---|---|---|
//! | `fit` (default) | `min(Wn/Wo, Hn/Ho)` | the old composition, scaled to sit whole inside the new canvas, centred |
//! | `fill` | `max(Wn/Wo, Hn/Ho)` | the old composition, scaled to cover the new canvas (edges may be cut) |
//! | `keep_scale` | `1` | every clip keeps its pixel size and its pixel offset from the centre |
//! | `none` | n/a | transforms are not touched |
//!
//! Every clip on a video track then gets, with its picture's aspect `a` (the media's upright
//! frame; the old canvas's aspect when the media has no picture size) and `fit(a, W, H)` the
//! pixel size of the aspect-fit picture:
//!
//! * `position.x' = position.x * Wo * k / Wn`, `position.y' = position.y * Ho * k / Hn`
//! * `scale' = scale * fit(a, Wo, Ho).width * k / fit(a, Wn, Hn).width` (both axes; the two
//!   fits keep the picture's aspect, so one factor serves both)
//!
//! Anchor, rotation and crop are in item units and stay. Keyed properties map every key's value
//! the same way (times and curves are kept). A clip left at the default transform with the same
//! aspect on both canvases keeps exactly that transform.
//!
//! Text and overlay items (later versions) will use the same position rule: their positions are
//! canvas fractions and scale proportionally by `Wo * k / Wn`.

use cutweave_commands::wire;
use cutweave_model::{Clip, MediaAsset, MediaKind, StreamKind, Vec2};

/// How clip transforms follow a new canvas (see the module docs).
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReframePolicy {
    /// Keep each clip's pixel size and pixel offset from the centre.
    KeepScale,
    /// Scale the composition to sit whole inside the new canvas.
    #[default]
    Fit,
    /// Scale the composition to cover the new canvas.
    Fill,
    /// Leave transforms alone.
    None,
}

/// A canvas size in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Canvas {
    /// Width in pixels.
    pub w: f64,
    /// Height in pixels.
    pub h: f64,
}

impl Canvas {
    /// A canvas from integer sizes.
    #[must_use]
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas {
            w: f64::from(w),
            h: f64::from(h),
        }
    }
}

/// The uniform pixel factor of `policy` from `old` to `new`, or `None` when transforms stay.
#[must_use]
pub fn factor(policy: ReframePolicy, old: Canvas, new: Canvas) -> Option<f64> {
    let (kx, ky) = (new.w / old.w, new.h / old.h);
    match policy {
        ReframePolicy::None => None,
        ReframePolicy::KeepScale => Some(1.0),
        ReframePolicy::Fit => Some(kx.min(ky)),
        ReframePolicy::Fill => Some(kx.max(ky)),
    }
}

/// The aspect (width over height) of the upright picture of `media`, if it has one.
#[must_use]
pub fn picture_aspect(media: &MediaAsset) -> Option<f64> {
    if media.kind == MediaKind::Audio {
        return None;
    }
    let stream = media
        .probe
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Video)?;
    let (w, h) = (f64::from(stream.width?), f64::from(stream.height?));
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(if matches!(media.probe.rotation, 90 | 270) {
        h / w
    } else {
        w / h
    })
}

/// The pixel size of a picture of aspect `a` fitted inside `canvas` (what `scale = 1` shows).
#[must_use]
pub fn fit_size(a: f64, canvas: Canvas) -> (f64, f64) {
    if a >= canvas.w / canvas.h {
        (canvas.w, canvas.w / a)
    } else {
        (canvas.h * a, canvas.h)
    }
}

/// The multipliers `(position.x, position.y, scale)` for a clip whose picture has aspect `a`.
#[must_use]
pub fn multipliers(k: f64, a: f64, old: Canvas, new: Canvas) -> (f64, f64, f64) {
    let (fo, _) = fit_size(a, old);
    let (fnew, _) = fit_size(a, new);
    (old.w * k / new.w, old.h * k / new.h, fo * k / fnew)
}

/// Applies the multipliers to a clip's position and scale (every key of a keyed property).
/// Returns whether anything changed.
pub fn reframe_clip(clip: &mut Clip, (mx, my, ms): (f64, f64, f64)) -> bool {
    if (mx - 1.0).abs() < f64::EPSILON
        && (my - 1.0).abs() < f64::EPSILON
        && (ms - 1.0).abs() < f64::EPSILON
    {
        return false;
    }
    let t = &mut clip.visual.transform;
    t.position = t.position.map_values(|p| Vec2::new(p.x * mx, p.y * my));
    t.scale = t.scale.map_values(|s| Vec2::new(s.x * ms, s.y * ms));
    true
}

/// Where a clip's picture lands on a canvas, in pixels: `(left, top, right, bottom)` of its
/// unrotated rectangle for a static transform. For tests and for UI previews of a reframe.
#[must_use]
pub fn placed_rect(a: f64, canvas: Canvas, position: Vec2, scale: Vec2, anchor: Vec2) -> [f64; 4] {
    let (fw, fh) = fit_size(a, canvas);
    let (w, h) = (fw * scale.x, fh * scale.y);
    let (px, py) = (
        canvas.w / 2.0 + position.x * canvas.w,
        canvas.h / 2.0 + position.y * canvas.h,
    );
    let (left, top) = (px - anchor.x * w, py - anchor.y * h);
    [left, top, left + w, top + h]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const OLD: Canvas = Canvas {
        w: 1920.0,
        h: 1080.0,
    };
    const NEW: Canvas = Canvas {
        w: 1080.0,
        h: 1920.0,
    };

    fn inside(r: [f64; 4], c: Canvas) -> bool {
        let eps = 1e-6;
        r[0] >= -eps && r[1] >= -eps && r[2] <= c.w + eps && r[3] <= c.h + eps
    }

    #[test]
    fn factors() {
        assert_eq!(factor(ReframePolicy::None, OLD, NEW), None);
        assert_eq!(factor(ReframePolicy::KeepScale, OLD, NEW), Some(1.0));
        assert_eq!(factor(ReframePolicy::Fit, OLD, NEW), Some(0.5625));
        let fill = factor(ReframePolicy::Fill, OLD, NEW).unwrap();
        assert!((fill - 1920.0 / 1080.0).abs() < 1e-12);
    }

    #[test]
    fn a_default_clip_of_the_canvas_aspect_keeps_its_transform_under_fit() {
        let a = 16.0 / 9.0;
        let k = factor(ReframePolicy::Fit, OLD, NEW).unwrap();
        let (mx, my, ms) = multipliers(k, a, OLD, NEW);
        assert!((ms - 1.0).abs() < 1e-12, "{ms}");
        assert_eq!(mx, 0.5625 * 1920.0 / 1080.0);
        assert_eq!(my, 0.5625 * 1080.0 / 1920.0);
    }

    #[test]
    fn fit_keeps_every_frame_visible_from_16_9_to_9_16() {
        // Clips of several aspects, at several scales and offsets that are inside the old canvas.
        let aspects = [16.0 / 9.0, 9.0 / 16.0, 1.0, 4.0 / 3.0, 21.0 / 9.0];
        let k = factor(ReframePolicy::Fit, OLD, NEW).unwrap();
        for a in aspects {
            for scale in [0.25, 0.5, 1.0] {
                for (px, py) in [(0.0, 0.0), (0.2, -0.1), (-0.25, 0.25)] {
                    let (position, s) = (Vec2::new(px, py), Vec2::new(scale, scale));
                    let anchor = Vec2::new(0.5, 0.5);
                    let before = placed_rect(a, OLD, position, s, anchor);
                    if !inside(before, OLD) {
                        continue;
                    }
                    let (mx, my, ms) = multipliers(k, a, OLD, NEW);
                    let after = placed_rect(
                        a,
                        NEW,
                        Vec2::new(px * mx, py * my),
                        Vec2::new(scale * ms, scale * ms),
                        anchor,
                    );
                    assert!(
                        inside(after, NEW),
                        "aspect {a} scale {scale} at ({px}, {py}): {after:?}"
                    );
                    // The whole composition shrank by exactly k.
                    assert!(((after[2] - after[0]) - (before[2] - before[0]) * k).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn fill_covers_the_new_canvas_with_a_full_frame_clip() {
        let a = 16.0 / 9.0;
        let k = factor(ReframePolicy::Fill, OLD, NEW).unwrap();
        let (mx, my, ms) = multipliers(k, a, OLD, NEW);
        let r = placed_rect(
            a,
            NEW,
            Vec2::new(0.0 * mx, 0.0 * my),
            Vec2::new(ms, ms),
            Vec2::new(0.5, 0.5),
        );
        assert!(r[0] <= 1e-6 && r[1] <= 1e-6 && r[2] >= NEW.w - 1e-6 && r[3] >= NEW.h - 1e-6);
    }

    #[test]
    fn keep_scale_keeps_pixel_size_and_offset() {
        let a = 16.0 / 9.0;
        let (mx, my, ms) = multipliers(1.0, a, OLD, NEW);
        let position = Vec2::new(0.1, -0.2);
        let before = placed_rect(a, OLD, position, Vec2::ONE, Vec2::new(0.5, 0.5));
        let after = placed_rect(
            a,
            NEW,
            Vec2::new(position.x * mx, position.y * my),
            Vec2::new(ms, ms),
            Vec2::new(0.5, 0.5),
        );
        // Same pixel size, same offset of the centre from the canvas centre.
        assert!(((after[2] - after[0]) - (before[2] - before[0])).abs() < 1e-6);
        let c = |r: [f64; 4], cv: Canvas| {
            (
                f64::midpoint(r[0], r[2]) - cv.w / 2.0,
                f64::midpoint(r[1], r[3]) - cv.h / 2.0,
            )
        };
        let (bx, by) = c(before, OLD);
        let (ax, ay) = c(after, NEW);
        assert!((bx - ax).abs() < 1e-6 && (by - ay).abs() < 1e-6);
    }
}
