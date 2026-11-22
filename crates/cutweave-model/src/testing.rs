// SPDX-License-Identifier: GPL-3.0-or-later
//! Proptest strategies for model types (feature `testing`). Reused by the property-test harness
//! of `cutweave-ops` (WP-1B.07).

use proptest::prelude::*;
use serde_json::json;

use crate::{
    Clip, Duration, FrameRate, IdGen, MAX_SAFE_FLICKS, MediaAsset, MediaKind, ProbeInfo, Project,
    SampleRate, SeededGen, Sequence, SequenceFormat, Time, Timecode, Track, TrackKind, Vec2,
};

/// Any representable [`Time`] (±(2⁵³ − 1) flicks).
pub fn arb_time() -> impl Strategy<Value = Time> {
    (-MAX_SAFE_FLICKS..=MAX_SAFE_FLICKS).prop_map(|f| Time::from_flicks(f).unwrap_or(Time::ZERO))
}

/// Any representable [`Duration`].
pub fn arb_duration() -> impl Strategy<Value = Duration> {
    (0..=MAX_SAFE_FLICKS).prop_map(|f| Duration::from_flicks(f).unwrap_or(Duration::ZERO))
}

/// One of the ADR-0004 frame rates ([`FrameRate::COMMON`]).
pub fn arb_frame_rate() -> impl Strategy<Value = FrameRate> {
    proptest::sample::select(FrameRate::COMMON.to_vec())
}

/// Any valid frame rate, including inexact ones (`num` up to 1,000,000, `den` up to 10,000).
pub fn arb_any_frame_rate() -> impl Strategy<Value = FrameRate> {
    (1..=1_000_000_u32, 1..=10_000_u32)
        .prop_filter_map("valid rate", |(num, den)| FrameRate::new(num, den).ok())
}

/// One of the ADR-0004 sample rates ([`SampleRate::COMMON`]).
pub fn arb_sample_rate() -> impl Strategy<Value = SampleRate> {
    proptest::sample::select(SampleRate::COMMON.to_vec())
}

/// A valid timecode label at `rate` (drop-frame when `drop_frame` and the rate has it), for
/// frames up to about 100 hours, paired with its frame index.
pub fn arb_timecode(rate: FrameRate, drop_frame: bool) -> impl Strategy<Value = (i64, Timecode)> {
    let drop_frame = drop_frame && rate.drop_frames_per_minute().is_some();
    let max_frame = i64::from(rate.nominal_fps()) * 3600 * 100;
    (0..max_frame).prop_filter_map("representable timecode", move |frame| {
        Timecode::from_frame(frame, rate, drop_frame)
            .ok()
            .map(|tc| (frame, tc))
    })
}

/// Flicks per frame at 30 fps, the grid of generated media durations.
const MEDIA_FRAME: i64 = 23_520_000;

#[derive(Clone, Debug)]
struct MediaRecipe {
    kind: MediaKind,
    frames: i64,
}

#[derive(Clone, Debug)]
struct ClipRecipe {
    gap_frames: i64,
    frames: i64,
    media: prop::sample::Index,
    source_frames: i64,
    opacity: f64,
    gain_db: f64,
    scale: f64,
    position: (f64, f64),
    enabled: bool,
}

#[derive(Clone, Debug)]
struct TrackRecipe {
    kind: TrackKind,
    muted: bool,
    hidden: bool,
    locked: bool,
    clips: Vec<ClipRecipe>,
}

#[derive(Clone, Debug)]
struct SequenceRecipe {
    rate: FrameRate,
    tracks: Vec<TrackRecipe>,
}

#[derive(Clone, Debug)]
struct ProjectRecipe {
    seed: u64,
    media: Vec<MediaRecipe>,
    sequences: Vec<SequenceRecipe>,
    link_first_pair: bool,
    plugin_data: bool,
}

fn media_recipe() -> impl Strategy<Value = MediaRecipe> {
    (
        prop_oneof![
            Just(MediaKind::Video),
            Just(MediaKind::Audio),
            Just(MediaKind::Image)
        ],
        1..=18_000_i64,
    )
        .prop_map(|(kind, frames)| MediaRecipe { kind, frames })
}

fn clip_recipe() -> impl Strategy<Value = ClipRecipe> {
    (
        (
            0..=90_i64,
            1..=900_i64,
            any::<prop::sample::Index>(),
            0..=18_000_i64,
        ),
        (0.0..=1.0_f64, -48.0..=12.0_f64, 0.1..=4.0_f64),
        ((-1.0..=1.0_f64), (-1.0..=1.0_f64), any::<bool>()),
    )
        .prop_map(
            |((gap_frames, frames, media, source_frames), (opacity, gain_db, scale), (x, y, e))| {
                ClipRecipe {
                    gap_frames,
                    frames,
                    media,
                    source_frames,
                    opacity,
                    gain_db,
                    scale,
                    position: (x, y),
                    enabled: e,
                }
            },
        )
}

fn track_recipe() -> impl Strategy<Value = TrackRecipe> {
    (
        prop_oneof![3 => Just(TrackKind::Video), 2 => Just(TrackKind::Audio)],
        any::<(bool, bool, bool)>(),
        prop::collection::vec(clip_recipe(), 0..6),
    )
        .prop_map(|(kind, (muted, hidden, locked), clips)| TrackRecipe {
            kind,
            muted,
            hidden,
            locked,
            clips,
        })
}

fn sequence_recipe() -> impl Strategy<Value = SequenceRecipe> {
    (
        arb_frame_rate(),
        prop::collection::vec(track_recipe(), 1..4),
    )
        .prop_map(|(rate, tracks)| SequenceRecipe { rate, tracks })
}

fn project_recipe() -> impl Strategy<Value = ProjectRecipe> {
    (
        any::<u64>(),
        prop::collection::vec(media_recipe(), 0..5),
        prop::collection::vec(sequence_recipe(), 0..3),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(seed, media, sequences, link_first_pair, plugin_data)| ProjectRecipe {
                seed,
                media,
                sequences,
                link_first_pair,
                plugin_data,
            },
        )
}

fn flicks(f: i64) -> Time {
    Time::from_flicks(f).unwrap_or(Time::ZERO)
}

fn span(f: i64) -> Duration {
    Duration::from_flicks(f).unwrap_or(Duration::ZERO)
}

fn build(recipe: &ProjectRecipe) -> Project {
    let mut ids = SeededGen::new(recipe.seed);
    let mut project = Project::new(format!("Generated {}", recipe.seed));
    for (n, m) in recipe.media.iter().enumerate() {
        let probe = ProbeInfo {
            duration: (m.kind != MediaKind::Image).then(|| span(m.frames * MEDIA_FRAME)),
            ..ProbeInfo::default()
        };
        let asset = MediaAsset::file(ids.mint(), m.kind, format!("/media/{n}.bin"), probe);
        project.media.insert(asset.id, asset);
    }
    let media: Vec<MediaAsset> = project.media.values().cloned().collect();
    for (n, s) in recipe.sequences.iter().enumerate() {
        let format = SequenceFormat::new(1920, 1080, s.rate, SampleRate::HZ_48_000);
        let mut sequence = Sequence::new(ids.mint(), format!("Sequence {n}"), format);
        let frame = s.rate.flicks_per_frame().unwrap_or(MEDIA_FRAME);
        for (t, tr) in s.tracks.iter().enumerate() {
            let mut track = Track::new(ids.mint(), tr.kind, format!("Track {t}"));
            track.muted = tr.muted;
            track.hidden = tr.hidden;
            track.locked = tr.locked;
            let usable: Vec<&MediaAsset> =
                media.iter().filter(|m| tr.kind.accepts(m.kind)).collect();
            let mut cursor = 0_i64;
            for c in &tr.clips {
                if usable.is_empty() {
                    break;
                }
                let asset = usable[c.media.index(usable.len())];
                let start = cursor + c.gap_frames * frame;
                let mut length = c.frames * frame;
                let mut source_in = 0;
                if let Some(total) = asset.probe.duration {
                    length = length.min(total.flicks());
                    source_in = (c.source_frames * frame).min(total.flicks() - length);
                }
                let mut clip = Clip::new(ids.mint(), asset.id, flicks(start), span(length));
                clip.source_in = flicks(source_in);
                clip.enabled = c.enabled;
                clip.audio.gain.set(c.gain_db);
                clip.visual.opacity.set(c.opacity);
                clip.visual.transform.scale.set(Vec2::new(c.scale, c.scale));
                clip.visual
                    .transform
                    .position
                    .set(Vec2::new(c.position.0, c.position.1));
                cursor = start + length;
                track.items.insert(clip.into());
            }
            sequence.insert_track(t, track);
        }
        project.sequences.insert(sequence.id, sequence);
    }
    if recipe.link_first_pair {
        link_first_video_and_audio(&mut project, &mut ids);
    }
    if recipe.plugin_data {
        let _ = project
            .custom
            .insert("com.example.test", json!({ "seed": recipe.seed }));
    }
    project
}

/// Links the first item of the first video track with the first item of the first audio track
/// of the first sequence, if both exist.
fn link_first_video_and_audio(project: &mut Project, ids: &mut SeededGen) {
    let Some(key) = project.sequences.keys().next().copied() else {
        return;
    };
    let Some(sequence) = project.sequences.get_mut(&key) else {
        return;
    };
    let first_of = |kind: TrackKind| {
        sequence
            .tracks_in_order()
            .filter(|t| t.kind == kind)
            .find_map(|t| t.items.iter().next().map(|i| (t.id, i.id())))
    };
    let (Some(video), Some(audio)) = (first_of(TrackKind::Video), first_of(TrackKind::Audio))
    else {
        return;
    };
    let group = ids.mint();
    for (track, item) in [video, audio] {
        if let Some(track) = sequence.track_mut(&track) {
            track
                .items
                .update(&item, |i| i.set_linked_group(Some(group)));
        }
    }
}

/// A valid schema v1 [`Project`] (`validate()` passes): up to 4 media files (video, audio,
/// image), up to 2 sequences at ADR-0004 frame rates, each with 1–3 video/audio tracks of
/// non-overlapping clips within their media bounds, optional linked items and plugin data. IDs
/// come from a [`SeededGen`] with a generated seed.
pub fn arb_project() -> impl Strategy<Value = Project> {
    project_recipe().prop_map(|recipe| build(&recipe))
}
