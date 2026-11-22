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
//! * Source time is [`Speed::map`] of the time since the clip start (identity speed in v1).
//! * Animatable values are evaluated at the clip-relative time, then sanitized (non-finite
//!   numbers fall back to the identity, opacity is clamped to `0..=1`) so the compositor never
//!   sees NaN.
//! * Anything that cannot be resolved (missing media, unrepresentable time) skips that clip
//!   instead of failing: evaluation of a valid project never panics and never errors on content.

use cutweave_model::{
    Clip, Duration, Item, MediaAsset, MediaKind, Project, Sequence, SequenceId, StreamKind, Time,
    Track, TrackKind, Vec2,
};

use crate::{
    scene::{
        AudioActivation, AudioSegment, BlendMode, Crop, FrameRequest, Layer, LayerTransform,
        SceneDescription,
    },
    speed::Speed,
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
    let format = &sequence.format;

    let mut layers = Vec::new();
    // `track_order` is top to bottom; compositing order is bottom first.
    let top_down: Vec<&Track> = sequence.tracks_in_order().collect();
    for track in top_down.into_iter().rev() {
        if track.kind != TrackKind::Video || track.hidden {
            continue;
        }
        if let Some(Item::Clip(clip)) = track.items.at(t)
            && let Some(layer) = layer_at(project, track, clip, t)
        {
            layers.push(layer);
        }
    }

    let solo = any_solo(sequence);
    let mut audio = Vec::new();
    for track in sequence.tracks_in_order() {
        if !audible(track, solo) {
            continue;
        }
        if let Some(Item::Clip(clip)) = track.items.at(t)
            && let Some(a) = activation_at(project, track, clip, t)
        {
            audio.push(a);
        }
    }

    Ok(SceneDescription {
        sequence: seq,
        time: t,
        width: format.width,
        height: format.height,
        frame_rate: format.frame_rate,
        background: format.background,
        layers,
        audio,
    })
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

/// Time since the clip start, as a duration and as an item-relative time for animatables.
fn offset_in(clip: &Clip, t: Time) -> Option<(Duration, Time)> {
    let offset = t.duration_since(clip.start).ok()?;
    if offset >= clip.duration {
        return None;
    }
    let rel = Time::from_flicks(offset.flicks()).ok()?;
    Some((offset, rel))
}

fn layer_at(project: &Project, track: &Track, clip: &Clip, t: Time) -> Option<Layer> {
    let media = media_of(project, clip)?;
    let still = match media.kind {
        MediaKind::Video => false,
        MediaKind::Image => true,
        // Compound clips need recursive evaluation (later); audio has no picture.
        MediaKind::Audio | MediaKind::SequenceRef => return None,
    };
    let (offset, rel) = offset_in(clip, t)?;
    let source_time = if still {
        Time::ZERO
    } else {
        Speed::NORMAL.map(clip.source_in, clip.duration, offset)?
    };
    let stream = first_stream(media, StreamKind::Video);

    let tr = &clip.visual.transform;
    Some(Layer {
        item: clip.id,
        track: track.id,
        request: FrameRequest {
            media: clip.media,
            stream: stream.map(|s| s.index),
            source_time,
            source_size: stream.and_then(|s| Some((s.width?, s.height?))),
            rotation: media.probe.rotation,
            color: media.probe.color.clone(),
            still,
            proxy: false,
        },
        transform: LayerTransform {
            position: finite_vec(*tr.position.value_at(rel), Vec2::ZERO),
            scale: finite_vec(*tr.scale.value_at(rel), Vec2::ONE),
            rotation_deg: finite(*tr.rotation.value_at(rel), 0.0),
            anchor: finite_vec(*tr.anchor.value_at(rel), Vec2::new(0.5, 0.5)),
        },
        opacity: finite(*clip.visual.opacity.value_at(rel), 1.0).clamp(0.0, 1.0),
        blend: BlendMode::Normal,
        crop: Crop::default(),
        effects: Vec::new(),
    })
}

fn activation_at(
    project: &Project,
    track: &Track,
    clip: &Clip,
    t: Time,
) -> Option<AudioActivation> {
    let media = media_of(project, clip)?;
    if !media.has_audio() || media.kind == MediaKind::SequenceRef {
        return None;
    }
    let (offset, rel) = offset_in(clip, t)?;
    let speed = Speed::NORMAL;
    Some(AudioActivation {
        item: clip.id,
        track: track.id,
        media: clip.media,
        stream: first_stream(media, StreamKind::Audio).map(|s| s.index),
        source_time: speed.map(clip.source_in, clip.duration, offset)?,
        clip_gain_db: finite(*clip.audio.gain.value_at(rel), 0.0),
        track_gain_db: finite(track.gain, 0.0),
        reverse: speed.reverse,
        speed: (speed.num, speed.den),
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
    if !media.has_audio() || media.kind == MediaKind::SequenceRef {
        return None;
    }
    let clip_end = clip.start.checked_add(clip.duration).ok()?;
    let dst_start = clip.start.max(start);
    let dst_end = clip_end.min(end);
    if dst_end <= dst_start {
        return None;
    }
    let speed = Speed::NORMAL;
    let (offset, rel) = offset_in(clip, dst_start)?;
    Some(AudioSegment {
        item: clip.id,
        track: track.id,
        media: clip.media,
        stream: first_stream(media, StreamKind::Audio).map(|s| s.index),
        dst_start,
        dst_duration: dst_end.duration_since(dst_start).ok()?,
        src_start: speed.map(clip.source_in, clip.duration, offset)?,
        clip_gain_db: finite(*clip.audio.gain.value_at(rel), 0.0),
        track_gain_db: finite(track.gain, 0.0),
        reverse: speed.reverse,
        speed: (speed.num, speed.den),
        sample_rate: sequence.format.sample_rate,
    })
}

fn finite(v: f64, fallback: f64) -> f64 {
    if v.is_finite() { v } else { fallback }
}

fn finite_vec(v: Vec2, fallback: Vec2) -> Vec2 {
    Vec2::new(finite(v.x, fallback.x), finite(v.y, fallback.y))
}
