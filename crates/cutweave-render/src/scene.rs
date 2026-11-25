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
    Animatable, Color, ColorTags, Duration, Fade, FrameRate, ItemId, MediaId, SampleRate,
    SequenceId, Time, TrackId, Vec2,
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
    /// Visible layers in compositing order: **first is the bottom**, last is drawn on top. The
    /// bottom-most video track comes first; the sequence's `track_order` (top to bottom) is
    /// walked in reverse.
    pub layers: Vec<Layer>,
    /// Audio clips sounding at `time`, in track order (top to bottom).
    pub audio: Vec<AudioActivation>,
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
    /// Crop of the source picture.
    pub crop: Crop,
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

/// Layer blend modes. Only `Normal` exists in schema v1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BlendMode {
    /// Straight-alpha "over".
    #[default]
    Normal,
}

impl BlendMode {
    /// The variant's name, as `Debug` prints it (hashed into cache keys without formatting).
    #[must_use]
    pub const fn debug_name(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
        }
    }
}

/// Crop of the source picture as fractions of its size removed from each edge.
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
    /// Constant speed factor `numerator / denominator` (1/1 in schema v1).
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
    /// Source position corresponding to `dst_start`.
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
    /// Constant speed factor `numerator / denominator` (1/1 in schema v1).
    pub speed: (u32, u32),
    /// Sample rate of the sequence the segment is mixed into.
    pub sample_rate: SampleRate,
}
