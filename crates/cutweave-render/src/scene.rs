// SPDX-License-Identifier: GPL-3.0-or-later
//! The scene description: the plain-data result of evaluating a timeline at one time
//! (arch §8). It is the input of the compositor (WP-1F.02) and of the audio mixer (WP-1G).
//!
//! Everything here is `serde` data with no behaviour, so scenes can be snapshotted, hashed and
//! sent over the wire.
//!
//! # Differences from the Spike S4 scene graph
//!
//! The S4 `Scene`/`Layer` (spikes/s4-compositor) is the shape the compositor executor was
//! proven on; this description is deliberately close (size, background, bottom-to-top layers,
//! per-layer transform / opacity / blend / effects, media frame requests in flicks). Two things
//! differ and the executor port must account for them:
//!
//! * **Transform units.** S4 positions layers in canvas *pixels* with the layer's intrinsic
//!   size. The model (arch §4.4) stores *normalized* values, and this description passes them
//!   through unchanged: `position` is in canvas units (1.0 = canvas width / height, offset of
//!   the anchor from the canvas center), `anchor` is in item units (0..1), `scale` multiplies
//!   the aspect-fit size. The compositor derives the S4 pixel transform from the canvas size
//!   and the source size ([`FrameRequest::source_size`], [`FrameRequest::rotation`]).
//! * **Layer content.** S4 has `Node` variants (media, shape, text, group, transition). In
//!   schema v1 the only content is a media frame, so [`Layer`] holds a [`FrameRequest`]
//!   directly; a `Node`-like enum arrives with text and compositions.

use cutweave_model::{
    Animatable, BackgroundFit, Color, ColorTags, Duration, Fade, FrameRate, ItemId, MediaId,
    SampleRate, SequenceId, Time, TrackId, Vec2,
};
use serde::Serialize;

/// A timeline evaluated at one time: what to draw and what to play.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SceneDescription {
    /// The evaluated sequence.
    pub sequence: SequenceId,
    /// The evaluated time (sequence time, as given).
    pub time: Time,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// Sequence frame rate.
    pub frame_rate: FrameRate,
    /// Canvas background (straight alpha, sRGB-encoded 8-bit as stored in the model).
    pub background: Color,
    /// What fills the canvas over the colour, behind all layers (WP-2B.02); `None`: the colour
    /// alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_node: Option<BackgroundNode>,
    /// Visible layers in compositing order: **first is the bottom**, last is drawn on top. The
    /// bottom-most video track comes first; the sequence's `track_order` (top to bottom) is
    /// walked in reverse.
    pub layers: Vec<Layer>,
    /// Audio clips sounding at `time`, in track order (top to bottom).
    pub audio: Vec<AudioActivation>,
}

/// The background fill of a scene (the rules are in [`crate::backdrop`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackgroundNode {
    /// A blurred copy of one layer's picture, scaled to cover the canvas.
    Blur {
        /// Index into [`SceneDescription::layers`] of the layer whose picture is blurred.
        layer: usize,
        /// The blur's standard deviation as a fraction of the canvas height.
        radius: f64,
        /// The share of the blurred picture's light taken away, `0..=1`.
        dim: f64,
    },
    /// A still image placed over the colour.
    Image {
        /// The image to decode (always the original, never a proxy).
        request: FrameRequest,
        /// How the image is sized to the canvas.
        fit: BackgroundFit,
    },
    /// A transparent canvas: the colour is not drawn.
    Transparent,
}

/// One visible picture to composite.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Layer {
    /// The item this layer comes from.
    pub item: ItemId,
    /// The track holding the item.
    pub track: TrackId,
    /// The source frame to decode.
    pub request: FrameRequest,
    /// Placement on the canvas, evaluated at `time`.
    pub transform: LayerTransform,
    /// Opacity in `0.0..=1.0`, evaluated at `time`.
    pub opacity: f64,
    /// How the layer combines with what is below.
    pub blend: BlendMode,
    /// Crop of the upright source picture, evaluated at `time`.
    pub crop: Crop,
    /// Mirroring of the item about its centre, crop included, evaluated at `time`.
    pub flip: Flip,
    /// Effects applied to the layer, in order (none in schema v1).
    pub effects: Vec<EffectRef>,
}

/// A request for one decoded source frame (resolved by the decoder / frame cache).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FrameRequest {
    /// The media to decode.
    pub media: MediaId,
    /// Index of the container video stream to decode (the first video stream), if known.
    pub stream: Option<u32>,
    /// Position in the media (from its start) of the frame to show. Not snapped to a frame:
    /// the media layer picks the frame through its index (arch §4.1, VFR-safe). Zero for stills.
    pub source_time: Time,
    /// Coded size of the stream in pixels, if probed (needed for aspect-fit).
    pub source_size: Option<(u32, u32)>,
    /// Display rotation of the media in degrees clockwise (0, 90, 180, 270), applied before
    /// aspect-fit.
    pub rotation: u16,
    /// Color tags of the video stream, for the input transform.
    pub color: ColorTags,
    /// Whether the media is a still image.
    pub still: bool,
    /// Whether a proxy (a smaller, faster-to-decode rendition) may be served instead of the
    /// original: set by the preview quality ladder ([`crate::Quality::apply`]), never by
    /// [`crate::evaluate`] (export and agent frames use originals). Layout does not change: it
    /// uses [`FrameRequest::source_size`], and a proxy only enters the display → texel map.
    #[serde(skip_serializing_if = "core::ops::Not::not")]
    pub proxy: bool,
    /// The longest side, in pixels, the whole upright picture is drawn at on the output, never
    /// more than the original has: what a proxy must hold to be served (WP-2F.02's resolution
    /// rule). `None`: not known, or any size will do. Set with `proxy` by
    /// [`crate::Quality::apply_for_output`]; layout never reads it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drawn_long_edge: Option<u32>,
}

/// Placement of a layer, in the model's normalized units (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LayerTransform {
    /// Offset of the anchor from the canvas center, in canvas units (+y down).
    pub position: Vec2,
    /// Size multiplier per axis (1.0 = aspect-fit into the canvas).
    pub scale: Vec2,
    /// Clockwise rotation in degrees.
    pub rotation_deg: f64,
    /// The item point that `position` places and rotation turns around, in item units.
    pub anchor: Vec2,
}

/// Layer blend modes: arch §8's core set (the extended set arrives in P4 as L1 plugins).
///
/// Every mode blends in linear light on premultiplied colour (the compositor's `Rgba16Float`
/// working space). With `cs`/`as` the layer's premultiplied colour and alpha after crop
/// coverage and opacity, `cb`/`ab` the canvas below, and `Cs = cs / as`, `Cb = cb / ab` the
/// straight colours, each channel is the W3C separable compositing formula
///
/// ```text
/// co = cs·(1 − ab) + cb·(1 − as) + as·ab·B(Cb, Cs)        ao = as + ab·(1 − as)
/// ```
///
/// with the mode's `B` ([`BlendMode::blend`]). `Normal` (`B = Cs`) reduces to "over",
/// `co = cs + cb·(1 − as)`, and never reads the straight colours. Where either side is
/// transparent the other shows unchanged, and opacity 0 leaves the canvas exactly as it was.
/// The result is linear in the layer's coverage × opacity `k`: `co(k) = cb + k·(co(1) − cb)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BlendMode {
    /// `B = Cs`: premultiplied "over".
    #[default]
    Normal,
    /// `B = Cb·Cs`: darkens; white is neutral.
    Multiply,
    /// `B = Cb + Cs − Cb·Cs`: lightens; black is neutral.
    Screen,
    /// `B = 2·Cb·Cs` where `Cb ≤ ½`, else `Cs + (2Cb − 1) − Cs·(2Cb − 1)` (hard light with the
    /// layers swapped): multiply in the backdrop's shadows, screen in its highlights.
    Overlay,
    /// `B = min(Cb, Cs)`.
    Darken,
    /// `B = max(Cb, Cs)`.
    Lighten,
    /// `B = min(Cb + Cs, 1)`: linear dodge, the sum clipped to white.
    Add,
    /// The W3C soft light: `Cb − (1 − 2Cs)·Cb·(1 − Cb)` where `Cs ≤ ½`, else
    /// `Cb + (2Cs − 1)·(D(Cb) − Cb)` with `D(Cb) = ((16Cb − 12)·Cb + 4)·Cb` where `Cb ≤ ¼`, else
    /// `√Cb`.
    SoftLight,
}

impl BlendMode {
    /// Every mode, in the model's order.
    pub const ALL: [BlendMode; 8] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::Add,
        BlendMode::SoftLight,
    ];

    /// The variant's name, as `Debug` prints it (hashed into cache keys without formatting).
    #[must_use]
    pub const fn debug_name(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
            BlendMode::Add => "Add",
            BlendMode::SoftLight => "SoftLight",
        }
    }

    /// The mode's id in the composite shader (`blend_channel` in `composite.wgsl`).
    #[must_use]
    pub const fn shader_id(self) -> u32 {
        match self {
            BlendMode::Normal => 0,
            BlendMode::Multiply => 1,
            BlendMode::Screen => 2,
            BlendMode::Overlay => 3,
            BlendMode::Darken => 4,
            BlendMode::Lighten => 5,
            BlendMode::Add => 6,
            BlendMode::SoftLight => 7,
        }
    }

    /// The mode's `B(Cb, Cs)` on one straight linear channel. The non-normal modes take both
    /// inputs clamped to `[0, 1]`, the SDR working range of v1 where their formulas are defined
    /// (soft light's `√Cb` needs `Cb ≥ 0`); `Normal` returns `Cs` unchanged.
    #[must_use]
    pub fn blend(self, cb: f64, cs: f64) -> f64 {
        let (b, s) = (cb.clamp(0.0, 1.0), cs.clamp(0.0, 1.0));
        match self {
            BlendMode::Normal => cs,
            BlendMode::Multiply => b * s,
            BlendMode::Screen => b + s - b * s,
            BlendMode::Overlay => {
                if b <= 0.5 {
                    2.0 * b * s
                } else {
                    let t = 2.0 * b - 1.0;
                    s + t - s * t
                }
            }
            BlendMode::Darken => b.min(s),
            BlendMode::Lighten => b.max(s),
            BlendMode::Add => (b + s).min(1.0),
            BlendMode::SoftLight => {
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

    /// Composites premultiplied `src` (coverage and opacity applied) onto premultiplied `dst`
    /// with this mode: the formula in the type's docs, in `f64`.
    #[must_use]
    pub fn composite(self, src: [f64; 4], dst: [f64; 4]) -> [f64; 4] {
        let (a_s, a_b) = (src[3], dst[3]);
        if self == BlendMode::Normal || a_s <= 0.0 || a_b <= 0.0 {
            return core::array::from_fn(|i| src[i] + dst[i] * (1.0 - a_s));
        }
        let mut out = [0.0; 4];
        for i in 0..3 {
            let b = self.blend(dst[i] / a_b, src[i] / a_s);
            out[i] = src[i] * (1.0 - a_b) + dst[i] * (1.0 - a_s) + a_s * a_b * b;
        }
        out[3] = a_s + a_b * (1.0 - a_s);
        out
    }
}

impl From<cutweave_model::BlendMode> for BlendMode {
    fn from(mode: cutweave_model::BlendMode) -> BlendMode {
        use cutweave_model::BlendMode as M;
        match mode {
            M::Normal => BlendMode::Normal,
            M::Multiply => BlendMode::Multiply,
            M::Screen => BlendMode::Screen,
            M::Overlay => BlendMode::Overlay,
            M::Darken => BlendMode::Darken,
            M::Lighten => BlendMode::Lighten,
            M::Add => BlendMode::Add,
            M::SoftLight => BlendMode::SoftLight,
        }
    }
}

/// Mirroring of a layer (keyframes §7): about the item's centre, after the crop and before the
/// transform, so flipping a cropped picture mirrors what is visible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Flip {
    /// Mirror left-right.
    pub horizontal: bool,
    /// Mirror top-bottom.
    pub vertical: bool,
}

/// Crop of the upright source picture (its display rotation applied) as fractions of its size
/// removed from each edge, before any flip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Crop {
    /// Fraction removed from the left edge.
    pub left: f64,
    /// Fraction removed from the top edge.
    pub top: f64,
    /// Fraction removed from the right edge.
    pub right: f64,
    /// Fraction removed from the bottom edge.
    pub bottom: f64,
}

/// A reference to an effect instance on a layer. No effect exists in schema v1; the type
/// reserves the slot so the compositor's layer signature does not change when effects land.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectRef {
    /// Effect package reference, e.g. `cutweave.fx.gaussian-blur@1`.
    pub effect: String,
}

/// A clip that is audible at the evaluated time.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AudioActivation {
    /// The item.
    pub item: ItemId,
    /// The (audio) track holding it.
    pub track: TrackId,
    /// The media to read samples from.
    pub media: MediaId,
    /// Index of the container audio stream (the first audio stream), if known.
    pub stream: Option<u32>,
    /// Source position at the evaluated time.
    pub source_time: Time,
    /// Clip gain in dB at the evaluated time.
    pub clip_gain_db: f64,
    /// Track gain in dB.
    pub track_gain_db: f64,
    /// Whether the source plays backwards.
    pub reverse: bool,
    /// Constant speed factor `numerator / denominator` (the clip's `speed`, WP-2D.01).
    pub speed: (u32, u32),
}

/// A contiguous stretch of one audible clip within a timeline range, for the mixer (1G).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AudioSegment {
    /// The item.
    pub item: ItemId,
    /// The (audio) track holding it.
    pub track: TrackId,
    /// The media to read samples from.
    pub media: MediaId,
    /// Index of the container audio stream (the first audio stream), if known.
    pub stream: Option<u32>,
    /// Where the segment starts on the timeline.
    pub dst_start: Time,
    /// Length on the timeline.
    pub dst_duration: Duration,
    /// Source position corresponding to `dst_start` (`cutweave_model::source_time`; for a
    /// reversed clip the latest position of the segment, which then plays backwards).
    pub src_start: Time,
    /// Clip gain in dB at `dst_start`.
    pub clip_gain_db: f64,
    /// Where the whole clip starts on the timeline (`dst_start` is later when the segment is
    /// the clip's tail), the origin of the item time the envelopes below use.
    pub clip_start: Time,
    /// Length of the whole clip on the timeline.
    pub clip_duration: Duration,
    /// The clip's gain in dB over item time (keyframed; WP-2E.01). Shared with the document.
    pub gain: Animatable<f64>,
    /// The clip's stereo position `-1..=1` over item time.
    pub pan: Animatable<f64>,
    /// Fade in at the clip's start.
    pub fade_in: Fade,
    /// Fade out at the clip's end.
    pub fade_out: Fade,
    /// Track gain in dB.
    pub track_gain_db: f64,
    /// Whether the source plays backwards (source time then decreases along the segment).
    pub reverse: bool,
    /// Constant speed factor `numerator / denominator` (the clip's `speed`, WP-2D.01): source
    /// time moves `num / den` times as fast as the timeline.
    pub speed: (u32, u32),
    /// Sample rate of the sequence the segment is mixed into.
    pub sample_rate: SampleRate,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use proptest::prelude::*;

    use super::BlendMode;

    const MODES: [BlendMode; 8] = BlendMode::ALL;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn each_formula_at_known_points() {
        use BlendMode::{Add, Darken, Lighten, Multiply, Normal, Overlay, Screen, SoftLight};
        // (mode, Cb, Cs, B) worked by hand from the formulas in the type's docs.
        let rows = [
            (Normal, 0.2, 0.7, 0.7),
            (Normal, 0.2, 1.5, 1.5),
            (Multiply, 0.5, 0.4, 0.2),
            (Screen, 0.5, 0.4, 0.7),
            (Overlay, 0.25, 0.6, 0.3),
            (Overlay, 0.75, 0.6, 0.8),
            (Overlay, 0.5, 0.6, 0.6),
            (Darken, 0.3, 0.6, 0.3),
            (Lighten, 0.3, 0.6, 0.6),
            (Add, 0.3, 0.6, 0.9),
            (Add, 0.7, 0.6, 1.0),
            // Cs ≤ ½: Cb − (1 − 2Cs)·Cb·(1 − Cb) = 0.4 − 0.6·0.4·0.6.
            (SoftLight, 0.4, 0.2, 0.256),
            // Cs > ½, Cb ≤ ¼: D = ((16·0.2 − 12)·0.2 + 4)·0.2 = 0.448; 0.2 + 0.6·0.248.
            (SoftLight, 0.2, 0.8, 0.3488),
            // Cs > ½, Cb > ¼: D = √0.64 = 0.8; 0.64 + 0.6·0.16.
            (SoftLight, 0.64, 0.8, 0.736),
            // Out-of-range inputs are clamped for every mode but normal.
            (Multiply, 1.5, -0.2, 0.0),
            (Screen, 2.0, 0.5, 1.0),
            (SoftLight, -0.1, 0.9, 0.0),
        ];
        for (mode, cb, cs, want) in rows {
            let got = mode.blend(cb, cs);
            assert!(
                close(got, want),
                "{mode:?}({cb}, {cs}) = {got}, want {want}"
            );
        }
    }

    #[test]
    fn neutral_sources_leave_the_backdrop_unchanged() {
        for i in 0..=20 {
            let cb = f64::from(i) / 20.0;
            assert!(close(BlendMode::Multiply.blend(cb, 1.0), cb));
            assert!(close(BlendMode::Screen.blend(cb, 0.0), cb));
            assert!(close(BlendMode::Darken.blend(cb, 1.0), cb));
            assert!(close(BlendMode::Lighten.blend(cb, 0.0), cb));
            assert!(close(BlendMode::Add.blend(cb, 0.0), cb));
            assert!(close(BlendMode::SoftLight.blend(cb, 0.5), cb));
            // Overlay is hard light with the layers swapped: a mid-grey backdrop shows the
            // source, and black and white backdrops stay.
            assert!(close(BlendMode::Overlay.blend(0.5, cb), cb));
            assert!(close(BlendMode::Overlay.blend(0.0, cb), 0.0));
            assert!(close(BlendMode::Overlay.blend(1.0, cb), 1.0));
        }
    }

    #[test]
    fn soft_light_is_continuous_at_its_seams() {
        let m = BlendMode::SoftLight;
        for i in 0..=10 {
            let cb = f64::from(i) / 10.0;
            assert!((m.blend(cb, 0.5) - m.blend(cb, 0.5 + 1e-12)).abs() < 1e-9);
        }
        // D(Cb) meets √Cb at ¼.
        assert!((m.blend(0.25, 1.0) - m.blend(0.25 + 1e-12, 1.0)).abs() < 1e-9);
    }

    #[test]
    fn opaque_layers_show_b_and_transparent_sides_pass_through() {
        let dst = [0.3, 0.6, 0.9, 1.0];
        let src = [0.5, 0.2, 0.8, 1.0];
        for mode in MODES {
            let out = mode.composite(src, dst);
            for c in 0..3 {
                assert!(close(out[c], mode.blend(dst[c], src[c])), "{mode:?}");
            }
            assert_eq!(out[3], 1.0);
            // A transparent source changes nothing; a transparent backdrop shows the source.
            assert_eq!(mode.composite([0.0; 4], dst), dst, "{mode:?}");
            let clear = [0.0; 4];
            assert_eq!(mode.composite(src, clear), src, "{mode:?}");
        }
    }

    #[test]
    fn normal_is_premultiplied_over() {
        let dst = [0.2, 0.3, 0.1, 0.8];
        let src = [0.25, 0.1, 0.05, 0.5];
        let out = BlendMode::Normal.composite(src, dst);
        for c in 0..4 {
            assert!(close(out[c], src[c] + dst[c] * 0.5));
        }
    }

    #[test]
    fn model_modes_map_one_to_one() {
        use cutweave_model::BlendMode as M;
        let model = [
            M::Normal,
            M::Multiply,
            M::Screen,
            M::Overlay,
            M::Darken,
            M::Lighten,
            M::Add,
            M::SoftLight,
        ];
        for (m, mode) in model.into_iter().zip(MODES) {
            assert_eq!(BlendMode::from(m), mode);
            // The same snake_case names on the wire.
            assert_eq!(
                serde_json::to_value(m).unwrap(),
                serde_json::to_value(mode).unwrap()
            );
            assert_eq!(mode.debug_name(), format!("{mode:?}"));
        }
        let mut ids: Vec<u32> = MODES.iter().map(|m| m.shader_id()).collect();
        ids.dedup();
        assert_eq!(ids, (0..8).collect::<Vec<u32>>());
    }

    fn rgba() -> impl Strategy<Value = [f64; 4]> {
        (0.0f64..=1.0, 0.0f64..=1.0, 0.0f64..=1.0, 0.0f64..=1.0)
            .prop_map(|(r, g, b, a)| [r * a, g * a, b * a, a])
    }

    proptest! {
        /// Coverage × opacity `k` scales the layer before blending; the result is the backdrop
        /// plus `k` times the full blend's change (what the GPU's "over" stage relies on).
        #[test]
        fn blending_is_linear_in_coverage_times_opacity(
            src in rgba(), dst in rgba(), k in 0.0f64..=1.0, m in 0usize..8,
        ) {
            let mode = MODES[m];
            let full = mode.composite(src, dst);
            let scaled = mode.composite(src.map(|c| c * k), dst);
            for c in 0..4 {
                prop_assert!((scaled[c] - (dst[c] + k * (full[c] - dst[c]))).abs() < 1e-12);
            }
        }

        /// Premultiplied inputs give premultiplied outputs in range.
        #[test]
        fn results_stay_premultiplied_and_in_range(src in rgba(), dst in rgba(), m in 0usize..8) {
            let out = MODES[m].composite(src, dst);
            prop_assert!((0.0..=1.0 + 1e-12).contains(&out[3]));
            for c in 0..3 {
                prop_assert!(out[c] >= -1e-12 && out[c] <= out[3] + 1e-12, "{out:?}");
            }
        }
    }
}
