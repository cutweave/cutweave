// SPDX-License-Identifier: GPL-3.0-or-later
//! Timeline evaluation: `(project snapshot, sequence, t) → SceneDescription` (arch §8).
//!
//! A pure function of its inputs: no I/O, no clock, no randomness, no state between calls.
//! Iteration order follows the model's ordered maps, so equal inputs give identical output.
//!
//! Rules (all per track, walking `track_order`):
//!
//! * A clip is active at `t` when `start <= t < start + duration` (end exclusive).
//! * Video: video tracks that are not hidden; enabled clips whose media exists and has pictures
//!   (video or still image). Compound (`sequence_ref`) clips are skipped until they are
//!   supported. Layers are ordered bottom first.
//! * Audio: audio tracks only (a clip's sound lives on its own audio track; linked video clips
//!   are silent), not muted, honouring solo; enabled clips whose media has sound.
//! * Source time is the model's one mapping, [`SourceMap::at`] of the time since the clip start
//!   (`cutweave_model::source_time`, WP-2D.01): speed, reverse and freeze frames. Freeze frames
//!   are silent.
//! * Visual properties (transform, opacity, crop, flip; keyframes §6 and §7) are evaluated by
//!   the keyframe core at the item-relative time `t − start` (so speed and slips never move
//!   them) and clamped to each property's evaluation range ([`PropId`]'s registry: opacity and
//!   crop edges to `0..=1`, scale to `≥ 0`), then sanitized (a non-finite number falls back to
//!   the identity) so the compositor never sees NaN. The blend mode is static. Values stay
//!   `f64` here; they become `f32` only at the GPU boundary.
//! * The background fill (WP-2B.02; [`crate::backdrop`]) is static. A blur names the layer it
//!   blurs: the lowest one showing, or the one on the main track (the magnetic track; without
//!   one, the lowest video track). With no such layer at `t`, or an image that is gone or is
//!   not a still, the scene has no fill and the colour shows.
//! * Anything that cannot be resolved (missing media, unrepresentable time) skips that clip
//!   instead of failing: evaluation of a valid project never panics and never errors on content.
//!
//! **Repeated evaluation** (the preview render thread, arch §10): [`TimelineIndex`] flattens a
//! sequence once per document revision into sorted arrays, and
//! [`TimelineIndex::evaluate_into`] then fills a reused [`SceneDescription`] with exactly
//! [`evaluate`]'s result and no heap allocation (the model's persistent trees allocate cursor
//! stacks when walked once a track outgrows one node).

use cutweave_model::{
    Animatable, BackgroundFill, BackgroundFit, BlurSource, Clip, Color, Duration, FrameRate, Item,
    MediaAsset, MediaKind, Project, PropId, Sequence, SequenceId, SourceMap, StreamKind, Time,
    Track, TrackId, TrackKind, Vec2, VisualProps,
};

use crate::scene::{
    AudioActivation, AudioSegment, BackgroundNode, Crop, Flip, FrameRequest, Layer, LayerTransform,
    SceneDescription,
};

/// Why a sequence could not be evaluated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvalError {
    /// The project has no sequence with this ID.
    UnknownSequence(SequenceId),
}

impl core::fmt::Display for EvalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EvalError::UnknownSequence(id) => write!(f, "unknown sequence {id}"),
        }
    }
}

impl core::error::Error for EvalError {}

/// A scene of `sequence` at `t` with no layers or audio yet.
fn empty_scene(seq: SequenceId, sequence: &Sequence, t: Time) -> SceneDescription {
    let format = &sequence.format;
    SceneDescription {
        sequence: seq,
        time: t,
        width: format.width,
        height: format.height,
        frame_rate: format.frame_rate,
        background: format.background,
        background_node: None,
        layers: Vec::new(),
        audio: Vec::new(),
    }
}

/// Evaluates sequence `seq` of `project` at sequence time `t`.
///
/// Times outside all content yield an empty scene (background only), never an error; an unknown
/// sequence is the only failure.
pub fn evaluate(
    project: &Project,
    seq: SequenceId,
    t: Time,
) -> Result<SceneDescription, EvalError> {
    let sequence = project
        .sequences
        .get(&seq)
        .ok_or(EvalError::UnknownSequence(seq))?;
    let mut scene = empty_scene(seq, sequence, t);

    // `track_order` is top to bottom; compositing order is bottom first.
    for track in sequence
        .track_order
        .iter()
        .rev()
        .filter_map(|id| sequence.tracks.get(id))
    {
        if track.kind != TrackKind::Video || track.hidden {
            continue;
        }
        if let Some(Item::Clip(clip)) = track.items.at(t)
            && let Some(layer) = layer_at(media_of(project, clip), track.id, clip, t)
        {
            scene.layers.push(layer);
        }
    }
    scene.background_node = node_at(fill_of(project, sequence).as_ref(), &scene.layers);

    let solo = any_solo(sequence);
    for track in sequence.tracks_in_order() {
        if !audible(track, solo) {
            continue;
        }
        if let Some(Item::Clip(clip)) = track.items.at(t)
            && let Some(a) = activation_at(media_of(project, clip), track, clip, t)
        {
            scene.audio.push(a);
        }
    }
    Ok(scene)
}

/// A sequence's background fill resolved against the project (once per document revision).
#[derive(Clone, Debug)]
enum Fill {
    /// Blur the layer on track `main`, or the lowest layer when `None`.
    Blur {
        main: Option<TrackId>,
        radius: f64,
        dim: f64,
    },
    Image {
        request: FrameRequest,
        fit: BackgroundFit,
    },
    Transparent,
}

/// The fill of `sequence`, if it has one that can draw.
fn fill_of(project: &Project, sequence: &Sequence) -> Option<Fill> {
    match sequence.format.background_fill.as_ref()? {
        BackgroundFill::Transparent => Some(Fill::Transparent),
        BackgroundFill::Blur {
            source,
            radius,
            dim,
        } => {
            let main = match source {
                BlurSource::Below => None,
                BlurSource::MainTrack => Some(main_track(sequence)?),
            };
            Some(Fill::Blur {
                main,
                radius: finite(*radius, 0.0).clamp(0.0, 1.0),
                dim: finite(*dim, 0.0).clamp(0.0, 1.0),
            })
        }
        BackgroundFill::Image { media, fit } => {
            let asset = project.media.get(media)?;
            if asset.kind != MediaKind::Image {
                return None;
            }
            let stream = first_stream(asset, StreamKind::Video);
            Some(Fill::Image {
                request: FrameRequest {
                    media: *media,
                    stream: stream.map(|s| s.index),
                    source_time: Time::ZERO,
                    source_size: stream.and_then(|s| Some((s.width?, s.height?))),
                    rotation: asset.probe.rotation,
                    color: asset.probe.color.clone(),
                    still: true,
                    proxy: false,
                    drawn_long_edge: None,
                },
                fit: *fit,
            })
        }
    }
}

/// The main track of `sequence` for a blurred background: the magnetic track, else the lowest
/// video track.
fn main_track(sequence: &Sequence) -> Option<TrackId> {
    let (mut magnetic, mut lowest) = (None, None);
    for track in sequence
        .tracks_in_order()
        .filter(|t| t.kind == TrackKind::Video)
    {
        lowest = Some(track.id);
        if track.magnetic && magnetic.is_none() {
            magnetic = Some(track.id);
        }
    }
    magnetic.or(lowest)
}

/// The background node of a scene with `layers` (bottom first). No heap allocation.
fn node_at(fill: Option<&Fill>, layers: &[Layer]) -> Option<BackgroundNode> {
    match fill? {
        Fill::Transparent => Some(BackgroundNode::Transparent),
        Fill::Image { request, fit } => Some(BackgroundNode::Image {
            request: request.clone(),
            fit: *fit,
        }),
        Fill::Blur { main, radius, dim } => {
            let layer = match main {
                None => (!layers.is_empty()).then_some(0)?,
                Some(track) => layers.iter().position(|l| l.track == *track)?,
            };
            Some(BackgroundNode::Blur {
                layer,
                radius: *radius,
                dim: *dim,
            })
        }
    }
}

/// One item of an [`IndexedTrack`].
#[derive(Clone, Debug)]
struct Entry {
    start: Time,
    /// `start + duration` in wide integers (an invalid document may overflow `Time`).
    end: i128,
    /// The clip and its media ([`media_of`]) when the item is a clip.
    clip: Option<(Clip, Option<MediaAsset>)>,
}

/// A track's items in `(start, id)` order, with what evaluation reads of them.
#[derive(Clone, Debug)]
struct IndexedTrack {
    id: TrackId,
    gain: f64,
    entries: Vec<Entry>,
}

impl IndexedTrack {
    fn new(project: &Project, track: &Track) -> IndexedTrack {
        let entries = track
            .items
            .iter()
            .map(|item| Entry {
                start: item.start(),
                end: i128::from(item.start().flicks()) + i128::from(item.duration().flicks()),
                clip: match item {
                    Item::Clip(clip) => Some((clip.clone(), media_of(project, clip).cloned())),
                    _ => None,
                },
            })
            .collect();
        IndexedTrack {
            id: track.id,
            gain: track.gain,
            entries,
        }
    }

    /// The item playing at `t`, as `TrackItems::at` finds it: the last one (in `(start, id)`
    /// order) starting at or before `t`, if `t` is before its end.
    fn at(&self, t: Time) -> Option<&Entry> {
        let after = self.entries.partition_point(|e| e.start <= t);
        let entry = self.entries.get(after.checked_sub(1)?)?;
        (i128::from(t.flicks()) < entry.end).then_some(entry)
    }
}

/// A sequence flattened for repeated evaluation (see the module docs): build it when the
/// document changes, then [`evaluate_into`](TimelineIndex::evaluate_into) any number of times.
#[derive(Clone, Debug)]
pub struct TimelineIndex {
    sequence: SequenceId,
    width: u32,
    height: u32,
    frame_rate: FrameRate,
    background: Color,
    fill: Option<Fill>,
    /// Visible video tracks, bottom first.
    video: Vec<IndexedTrack>,
    /// Audible audio tracks (mute and solo applied), top first.
    audio: Vec<IndexedTrack>,
}

impl TimelineIndex {
    /// Indexes sequence `seq` of `project` (allocates; once per document revision).
    pub fn new(project: &Project, seq: SequenceId) -> Result<TimelineIndex, EvalError> {
        let sequence = project
            .sequences
            .get(&seq)
            .ok_or(EvalError::UnknownSequence(seq))?;
        let format = &sequence.format;
        let video = sequence
            .track_order
            .iter()
            .rev()
            .filter_map(|id| sequence.tracks.get(id))
            .filter(|track| track.kind == TrackKind::Video && !track.hidden)
            .map(|track| IndexedTrack::new(project, track))
            .collect();
        let solo = any_solo(sequence);
        let audio = sequence
            .tracks_in_order()
            .filter(|track| audible(track, solo))
            .map(|track| IndexedTrack::new(project, track))
            .collect();
        Ok(TimelineIndex {
            sequence: seq,
            width: format.width,
            height: format.height,
            frame_rate: format.frame_rate,
            background: format.background,
            fill: fill_of(project, sequence),
            video,
            audio,
        })
    }

    /// The indexed sequence.
    #[must_use]
    pub fn sequence(&self) -> SequenceId {
        self.sequence
    }

    /// The scene at `t`: the same as [`evaluate`] on the indexed project.
    #[must_use]
    pub fn evaluate(&self, t: Time) -> SceneDescription {
        let mut scene = SceneDescription {
            sequence: self.sequence,
            time: t,
            width: self.width,
            height: self.height,
            frame_rate: self.frame_rate,
            background: self.background,
            background_node: None,
            layers: Vec::new(),
            audio: Vec::new(),
        };
        self.evaluate_into(t, &mut scene);
        scene
    }

    /// [`TimelineIndex::evaluate`] into `scene`, replacing its content and reusing its
    /// vectors: no heap allocation once they have grown to the most layers and sounds seen.
    pub fn evaluate_into(&self, t: Time, scene: &mut SceneDescription) {
        scene.sequence = self.sequence;
        scene.time = t;
        scene.width = self.width;
        scene.height = self.height;
        scene.frame_rate = self.frame_rate;
        scene.background = self.background;
        scene.layers.clear();
        scene.audio.clear();
        for track in &self.video {
            if let Some(Entry {
                clip: Some((clip, media)),
                ..
            }) = track.at(t)
                && let Some(layer) = layer_at(media.as_ref(), track.id, clip, t)
            {
                scene.layers.push(layer);
            }
        }
        scene.background_node = node_at(self.fill.as_ref(), &scene.layers);
        for track in &self.audio {
            if let Some(Entry {
                clip: Some((clip, media)),
                ..
            }) = track.at(t)
                && let Some(a) = activation_on(media.as_ref(), (track.id, track.gain), clip, t)
            {
                scene.audio.push(a);
            }
        }
    }
}

/// The audible stretches of clips within the timeline range `[start, end)`, in track order
/// (top to bottom) then start order. Shared by the mixer (WP-1G).
pub fn evaluate_audio(
    project: &Project,
    seq: SequenceId,
    start: Time,
    end: Time,
) -> Result<Vec<AudioSegment>, EvalError> {
    let sequence = project
        .sequences
        .get(&seq)
        .ok_or(EvalError::UnknownSequence(seq))?;
    let solo = any_solo(sequence);
    let mut out = Vec::new();
    if end <= start {
        return Ok(out);
    }
    for track in sequence.tracks_in_order() {
        if !audible(track, solo) {
            continue;
        }
        for item in track.items.overlapping(start, end) {
            let Item::Clip(clip) = item else { continue };
            if let Some(segment) = segment_in(project, sequence, track, clip, start, end) {
                out.push(segment);
            }
        }
    }
    Ok(out)
}

fn any_solo(sequence: &Sequence) -> bool {
    sequence
        .tracks_in_order()
        .any(|t| t.kind == TrackKind::Audio && t.solo)
}

fn audible(track: &Track, any_solo: bool) -> bool {
    track.kind == TrackKind::Audio && !track.muted && (!any_solo || track.solo)
}

/// The enabled clip's media, if the clip can be evaluated against it.
fn media_of<'a>(project: &'a Project, clip: &Clip) -> Option<&'a MediaAsset> {
    if !clip.enabled {
        return None;
    }
    project.media.get(&clip.media)
}

fn first_stream(media: &MediaAsset, kind: StreamKind) -> Option<&cutweave_model::StreamInfo> {
    media.probe.streams.iter().find(|s| s.kind == kind)
}

/// The media time `clip` shows and plays `offset` into it (the model's mapping, WP-2D.01).
fn source_at(clip: &Clip, offset: Duration) -> Option<Time> {
    Time::from_flicks(SourceMap::of(clip).at(offset.flicks())?).ok()
}

/// The clip's speed for the mixer: `(num, den)`.
fn speed_of(clip: &Clip) -> (u32, u32) {
    (clip.speed.num(), clip.speed.den())
}

/// Time since the clip start, as a duration and as an item-relative time for animatables.
fn offset_in(clip: &Clip, t: Time) -> Option<(Duration, Time)> {
    let offset = t.duration_since(clip.start).ok()?;
    if offset >= clip.duration {
        return None;
    }
    let rel = Time::from_flicks(offset.flicks()).ok()?;
    Some((offset, rel))
}

/// The layer `clip` (on track `track`, showing `media`: [`media_of`]) contributes at `t`.
fn layer_at(media: Option<&MediaAsset>, track: TrackId, clip: &Clip, t: Time) -> Option<Layer> {
    let media = media?;
    let still = match media.kind {
        // An image sequence plays like video: frame `n` is its own picture (ADR-0035).
        MediaKind::Video | MediaKind::ImageSequence => false,
        MediaKind::Image => true,
        // Compound clips need recursive evaluation (later); audio has no picture.
        MediaKind::Audio | MediaKind::SequenceRef => return None,
    };
    let (offset, rel) = offset_in(clip, t)?;
    let source_time = if still {
        Time::ZERO
    } else {
        source_at(clip, offset)?
    };
    let stream = first_stream(media, StreamKind::Video);
    let visual = &clip.visual;
    let (transform, opacity, crop, flip) = visual_at(visual, rel);
    Some(Layer {
        item: clip.id,
        track,
        request: FrameRequest {
            media: clip.media,
            stream: stream.map(|s| s.index),
            source_time,
            source_size: stream.and_then(|s| Some((s.width?, s.height?))),
            rotation: media.probe.rotation,
            color: media.probe.color.clone(),
            still,
            proxy: false,
            drawn_long_edge: None,
        },
        transform,
        opacity,
        blend: visual.blend_mode.into(),
        crop,
        flip,
        effects: Vec::new(),
    })
}

/// A scalar property at item time `rel`, clamped to its evaluation range (keyframes §7) and
/// sanitized.
fn scalar_at(anim: &Animatable<f64>, id: PropId, rel: Time, fallback: f64) -> f64 {
    finite(anim.eval_clamped(rel, id.spec().clamp), fallback)
}

/// A vector property at item time `rel`, clamped and sanitized like [`scalar_at`].
fn vec_at(anim: &Animatable<Vec2>, id: PropId, rel: Time, fallback: Vec2) -> Vec2 {
    finite_vec(anim.eval_clamped(rel, id.spec().clamp), fallback)
}

/// The animated visual properties at item time `rel`: transform, opacity, crop and flip. No
/// heap allocation (the keyframe core evaluates scalars, vectors and booleans in place).
fn visual_at(visual: &VisualProps, rel: Time) -> (LayerTransform, f64, Crop, Flip) {
    let tr = &visual.transform;
    let transform = LayerTransform {
        position: vec_at(&tr.position, PropId::TransformPosition, rel, Vec2::ZERO),
        scale: vec_at(&tr.scale, PropId::TransformScale, rel, Vec2::ONE),
        rotation_deg: scalar_at(&tr.rotation, PropId::TransformRotation, rel, 0.0),
        anchor: vec_at(
            &tr.anchor,
            PropId::TransformAnchor,
            rel,
            Vec2::new(0.5, 0.5),
        ),
    };
    // A non-finite opacity hides nothing (the identity), as before keyframes; the clamp keeps
    // it in `0..=1`.
    let opacity = scalar_at(&visual.opacity, PropId::Opacity, rel, 1.0).clamp(0.0, 1.0);
    let c = &visual.crop;
    let crop = Crop {
        left: scalar_at(&c.left, PropId::CropLeft, rel, 0.0),
        top: scalar_at(&c.top, PropId::CropTop, rel, 0.0),
        right: scalar_at(&c.right, PropId::CropRight, rel, 0.0),
        bottom: scalar_at(&c.bottom, PropId::CropBottom, rel, 0.0),
    };
    let flip = Flip {
        horizontal: visual.flip.horizontal.eval(rel),
        vertical: visual.flip.vertical.eval(rel),
    };
    (transform, opacity, crop, flip)
}

fn activation_at(
    media: Option<&MediaAsset>,
    track: &Track,
    clip: &Clip,
    t: Time,
) -> Option<AudioActivation> {
    activation_on(media, (track.id, track.gain), clip, t)
}

/// The sound `clip` (on the track `(id, gain)`, playing `media`: [`media_of`]) makes at `t`.
fn activation_on(
    media: Option<&MediaAsset>,
    (track, gain): (TrackId, f64),
    clip: &Clip,
    t: Time,
) -> Option<AudioActivation> {
    let media = media?;
    if !media.has_audio() || media.kind == MediaKind::SequenceRef || clip.freeze.is_some() {
        return None;
    }
    let (offset, rel) = offset_in(clip, t)?;
    Some(AudioActivation {
        item: clip.id,
        track,
        media: clip.media,
        stream: first_stream(media, StreamKind::Audio).map(|s| s.index),
        source_time: source_at(clip, offset)?,
        clip_gain_db: finite(*clip.audio.gain.value_at(rel), 0.0),
        track_gain_db: finite(gain, 0.0),
        reverse: clip.reverse,
        speed: speed_of(clip),
    })
}

fn segment_in(
    project: &Project,
    sequence: &Sequence,
    track: &Track,
    clip: &Clip,
    start: Time,
    end: Time,
) -> Option<AudioSegment> {
    let media = media_of(project, clip)?;
    if !media.has_audio() || media.kind == MediaKind::SequenceRef || clip.freeze.is_some() {
        return None;
    }
    let clip_end = clip.start.checked_add(clip.duration).ok()?;
    let dst_start = clip.start.max(start);
    let dst_end = clip_end.min(end);
    if dst_end <= dst_start {
        return None;
    }
    let (offset, rel) = offset_in(clip, dst_start)?;
    Some(AudioSegment {
        item: clip.id,
        track: track.id,
        media: clip.media,
        stream: first_stream(media, StreamKind::Audio).map(|s| s.index),
        dst_start,
        dst_duration: dst_end.duration_since(dst_start).ok()?,
        src_start: source_at(clip, offset)?,
        clip_gain_db: finite(*clip.audio.gain.value_at(rel), 0.0),
        clip_start: clip.start,
        clip_duration: clip.duration,
        gain: clip.audio.gain.clone(),
        pan: clip.audio.pan.clone(),
        fade_in: clip.audio.fade_in,
        fade_out: clip.audio.fade_out,
        track_gain_db: finite(track.gain, 0.0),
        reverse: clip.reverse,
        speed: speed_of(clip),
        sample_rate: sequence.format.sample_rate,
    })
}

fn finite(v: f64, fallback: f64) -> f64 {
    if v.is_finite() { v } else { fallback }
}

fn finite_vec(v: Vec2, fallback: Vec2) -> Vec2 {
    Vec2::new(finite(v.x, fallback.x), finite(v.y, fallback.y))
}
