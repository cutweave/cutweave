// SPDX-License-Identifier: GPL-3.0-or-later
//! What a rendered frame shows, as structured data for agents (arch §14: "items visible"):
//! which items contribute pixels, where they land on the image, and why the others do not.
//!
//! Derived from the [`SceneDescription`] (the [`evaluate`](crate::evaluate) output) and the
//! [`RenderGraph`] compiled for the image, so it states exactly what the compositor drew:
//!
//! * **Visible** items are the layers with a composite pass, listed **top first** (the timeline
//!   UI's track order), each with the axis-aligned bounding box of its visible (cropped) picture
//!   on the image ([`PixelRect`], image pixels, clipped to the image, rounded outwards).
//! * **Hidden** items are the other layers: `unavailable` (no frame: offline media, decode
//!   failure), `transparent` (opacity 0), `offscreen` (outside the image or degenerate), or
//!   `occluded` (entirely under an opaque layer covering the image, never decoded).

use cutweave_commands::wire;
use cutweave_model::{ItemId, MediaId, Time, TrackId};

use crate::geometry::LayerGeometry;
use crate::graph::{PassOp, RenderGraph};
use crate::scene::SceneDescription;

/// A rectangle in image pixels: `x, y` of the top-left corner, then the size.
#[wire]
#[derive(Copy, Default, Eq, Hash)]
pub struct PixelRect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl PixelRect {
    /// The pixels touched by the float box `[x0, y0, x1, y1)`, clipped to a `size` image.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn enclosing(bounds: [f64; 4], size: (u32, u32)) -> PixelRect {
        let clip = |v: f64, max: u32| {
            if v.is_finite() {
                v.clamp(0.0, f64::from(max)) as u32
            } else {
                0
            }
        };
        let x0 = clip(bounds[0].floor(), size.0);
        let y0 = clip(bounds[1].floor(), size.1);
        let x1 = clip(bounds[2].ceil(), size.0).max(x0);
        let y1 = clip(bounds[3].ceil(), size.1).max(y0);
        PixelRect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    }
}

/// An item that contributes pixels to the image.
#[wire]
pub struct VisibleItem {
    /// The item.
    pub item: ItemId,
    /// Its track.
    pub track: TrackId,
    /// The media it shows.
    pub media: MediaId,
    /// Position in the media of the frame shown (flicks).
    pub source_time: Time,
    /// Bounding box of its visible picture on the image (image pixels, clipped).
    pub rect: PixelRect,
    /// Opacity (0..1].
    pub opacity: f64,
}

/// Why an active item contributes no pixels.
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HiddenReason {
    /// Entirely under an opaque layer that covers the image.
    Occluded,
    /// Outside the image, or scaled or cropped to nothing.
    Offscreen,
    /// Opacity 0.
    Transparent,
    /// Its frame could not be produced (offline media, decode failure).
    Unavailable,
}

/// An item active at the frame time that contributes no pixels.
#[wire]
pub struct HiddenItem {
    /// The item.
    pub item: ItemId,
    /// Its track.
    pub track: TrackId,
    /// Why it is not visible.
    pub reason: HiddenReason,
}

/// The items of `scene` as drawn by `graph` (compiled from `scene`): visible ones top first,
/// then hidden ones top first.
#[must_use]
pub fn frame_items(
    scene: &SceneDescription,
    graph: &RenderGraph,
) -> (Vec<VisibleItem>, Vec<HiddenItem>) {
    let output = graph.target.size();
    let mut visible = Vec::new();
    let mut hidden = Vec::new();
    for layer in scene.layers.iter().rev() {
        let composite = graph.passes.iter().find_map(|p| match &p.op {
            PassOp::Composite(c) if c.item == layer.item => Some(c),
            _ => None,
        });
        if let Some(c) = composite {
            visible.push(VisibleItem {
                item: layer.item,
                track: layer.track,
                media: layer.request.media,
                source_time: layer.request.source_time,
                rect: PixelRect::enclosing(c.geometry.bounds, output),
                opacity: c.opacity,
            });
            continue;
        }
        let reason = if graph.missing.iter().any(|(item, _)| *item == layer.item) {
            HiddenReason::Unavailable
        } else if layer.opacity.is_nan() || layer.opacity <= 0.0 {
            HiddenReason::Transparent
        } else {
            // Occluded layers were never decoded, so place them with the probed size (or the
            // canvas size when unprobed, which only matters for the offscreen test).
            let canvas = (scene.width, scene.height);
            let frame = layer.request.source_size.unwrap_or(canvas);
            match LayerGeometry::new(layer, canvas, output, frame) {
                Some(g) if !g.offscreen(output) => HiddenReason::Occluded,
                _ => HiddenReason::Offscreen,
            }
        };
        hidden.push(HiddenItem {
            item: layer.item,
            track: layer.track,
            reason,
        });
    }
    (visible, hidden)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::assert_is_empty)]

    use cutweave_model::{Color, ColorTags, FrameRate, SequenceId, Vec2};

    use super::*;
    use crate::frame::PixelFormat;
    use crate::graph::{GraphOptions, Target, build};
    use crate::scene::{BlendMode, Crop, Flip, FrameRequest, Layer, LayerTransform};
    use crate::testcard::{Alpha, Pattern, SyntheticSource, TestCard};

    fn layer(n: u128, media: u128) -> Layer {
        Layer {
            item: ItemId::from_u128(n),
            track: TrackId::from_u128(n),
            request: FrameRequest {
                media: MediaId::from_u128(media),
                stream: Some(0),
                source_time: Time::from_flicks(7).unwrap(),
                source_size: Some((1920, 1080)),
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

    fn source() -> SyntheticSource {
        let card =
            |format| TestCard::new(192, 108, Pattern::ColorBars, format, ColorTags::default());
        SyntheticSource::new()
            .with(MediaId::from_u128(10), card(PixelFormat::Nv12))
            .with(
                MediaId::from_u128(11),
                TestCard {
                    alpha: Alpha::Constant(0.5),
                    ..card(PixelFormat::Rgba8)
                },
            )
    }

    fn scene(layers: Vec<Layer>) -> SceneDescription {
        SceneDescription {
            sequence: SequenceId::from_u128(1),
            time: Time::ZERO,
            width: 1920,
            height: 1080,
            frame_rate: FrameRate::FPS_24,
            background: Color::BLACK,
            layers,
            audio: Vec::new(),
        }
    }

    #[test]
    fn enclosing_rects_round_outwards_and_clip() {
        let r = PixelRect::enclosing([10.2, 5.0, 20.0, 9.5], (100, 100));
        assert_eq!((r.x, r.y, r.width, r.height), (10, 5, 10, 5));
        let r = PixelRect::enclosing([-30.0, -1.0, 50.5, 300.0], (40, 200));
        assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 40, 200));
        let r = PixelRect::enclosing([90.0, 90.0, 80.0, 95.0], (100, 100));
        assert_eq!((r.width, r.height), (0, 5));
        let r = PixelRect::enclosing([f64::NAN, 0.0, 1.0, 1.0], (10, 10));
        assert_eq!(r.x, 0);
    }

    #[test]
    fn rects_map_canvas_placement_to_image_pixels() {
        // Bottom: full frame. Top: half size in the top-right quadrant (anchor top-right).
        let full = layer(1, 10);
        let mut pip = layer(2, 10);
        pip.transform.scale = Vec2::new(0.5, 0.5);
        pip.transform.anchor = Vec2::new(1.0, 0.0);
        pip.transform.position = Vec2::new(0.5, -0.5);
        let s = scene(vec![full, pip]);
        // Rendered at 768×432 (sequence 1920×1080 scaled by 0.4).
        let g = build(
            &s,
            Target::rgba8(768, 432),
            &source(),
            &GraphOptions::default(),
        )
        .unwrap();
        let (visible, hidden) = frame_items(&s, &g);
        assert!(hidden.is_empty());
        let ids: Vec<ItemId> = visible.iter().map(|v| v.item).collect();
        assert_eq!(
            ids,
            [ItemId::from_u128(2), ItemId::from_u128(1)],
            "top first"
        );
        assert_eq!(
            visible[0].rect,
            PixelRect {
                x: 384,
                y: 0,
                width: 384,
                height: 216
            }
        );
        assert_eq!(
            visible[1].rect,
            PixelRect {
                x: 0,
                y: 0,
                width: 768,
                height: 432
            }
        );
        assert_eq!(visible[0].source_time, Time::from_flicks(7).unwrap());
        assert_eq!(visible[0].media, MediaId::from_u128(10));
    }

    #[test]
    fn hidden_items_say_why() {
        let bottom = layer(1, 10); // under the opaque cover
        let cover = layer(5, 10);
        let mut away = layer(2, 10);
        away.transform.position = Vec2::new(3.0, 0.0);
        let mut faded = layer(3, 10);
        faded.opacity = 0.0;
        let offline = layer(4, 99);
        let mut glass = layer(6, 11); // semi-transparent RGBA on top
        glass.transform.scale = Vec2::new(0.25, 0.25);
        let s = scene(vec![bottom, cover, away, faded, offline, glass]);
        let g = build(
            &s,
            Target::rgba8(480, 270),
            &source(),
            &GraphOptions::default(),
        )
        .unwrap();
        let (visible, hidden) = frame_items(&s, &g);
        let ids: Vec<u128> = visible.iter().map(|v| v.item.as_u128()).collect();
        assert_eq!(ids, [6, 5]);
        let reasons: Vec<(u128, HiddenReason)> = hidden
            .iter()
            .map(|h| (h.item.as_u128(), h.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                (4, HiddenReason::Unavailable),
                (3, HiddenReason::Transparent),
                (2, HiddenReason::Offscreen),
                (1, HiddenReason::Occluded),
            ]
        );
        // The quarter-size top layer is centred: 120×67.5 px at (180, 101.25).
        assert_eq!(
            visible[0].rect,
            PixelRect {
                x: 180,
                y: 101,
                width: 120,
                height: 68
            }
        );
    }
}
