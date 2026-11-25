// SPDX-License-Identifier: GPL-3.0-or-later
//! Unit, property and snapshot tests for timeline evaluation.
#![allow(
    clippy::many_single_char_names,
    clippy::cast_possible_wrap,
    clippy::assert_is_empty
)]

use cutweave_model::{
    Animatable, Clip, Color, Duration, Easing, Extra, FrameRate, Interp, Item, ItemId, Keyframe,
    Keyframes, MediaAsset, MediaId, MediaKind, ProbeInfo, Project, SampleRate, Sequence,
    SequenceFormat, SequenceId, StreamInfo, StreamKind, Time, Track, TrackId, TrackKind, Vec2,
};
use proptest::prelude::*;

use crate::{
    BlendMode, Crop, EvalError, Flip, SceneDescription, TimelineIndex, evaluate, evaluate_audio,
};

const S: i64 = 705_600_000;

fn t(flicks: i64) -> Time {
    Time::from_flicks(flicks).unwrap()
}
fn sec(s: i64) -> Time {
    t(s * S)
}

fn stream(index: u32, kind: StreamKind) -> StreamInfo {
    StreamInfo {
        index,
        kind,
        codec: None,
        profile: None,
        width: (kind == StreamKind::Video).then_some(1920),
        height: (kind == StreamKind::Video).then_some(1080),
        pixel_format: None,
        sample_rate: None,
        channels: None,
        language: None,
        extra: Extra::new(),
    }
}

/// A hand-built project with one sequence (`seq`, 1920x1080 @ 24).
struct Fixture {
    project: Project,
    seq: SequenceId,
    next: u128,
}

impl Fixture {
    fn new() -> Fixture {
        let seq = SequenceId::from_u128(1);
        let mut format = SequenceFormat::new(1920, 1080, FrameRate::FPS_24, SampleRate::HZ_48_000);
        format.background = Color::rgb(10, 20, 30);
        let mut project = Project::new("fixture");
        project
            .sequences
            .insert(seq, Sequence::new(seq, "Main", format));
        Fixture {
            project,
            seq,
            next: 100,
        }
    }

    fn media(&mut self, n: u128, kind: MediaKind, secs: i64) -> MediaId {
        let id = MediaId::from_u128(n);
        let mut probe = ProbeInfo::default();
        if kind != MediaKind::Audio {
            probe.streams.push(stream(0, StreamKind::Video));
        }
        if kind != MediaKind::Image {
            probe.streams.push(stream(1, StreamKind::Audio));
        }
        if kind != MediaKind::Image {
            probe.duration = Some(Duration::from_secs(secs).unwrap());
        }
        self.project
            .media
            .insert(id, MediaAsset::file(id, kind, format!("/media/{n}"), probe));
        id
    }

    /// Adds a track at the bottom (so the first added is the lowest, the last the topmost
    /// when inserted at 0 — here every track goes on top).
    fn track(&mut self, n: u128, kind: TrackKind) -> TrackId {
        let id = TrackId::from_u128(n);
        let seq = self.project.sequences.get_mut(&self.seq).unwrap();
        seq.insert_track(0, Track::new(id, kind, format!("T{n}")));
        id
    }

    fn clip(&mut self, track: TrackId, media: MediaId, start: Time, dur: Duration) -> ItemId {
        let id = ItemId::from_u128(self.next);
        self.next += 1;
        let clip = Clip::new(id, media, start, dur);
        self.edit_track(track, |tr| {
            tr.items.insert(clip.into());
        });
        id
    }

    fn edit_track(&mut self, track: TrackId, f: impl FnOnce(&mut Track)) {
        let seq = self.project.sequences.get_mut(&self.seq).unwrap();
        f(seq.track_mut(&track).unwrap());
    }

    fn edit_clip(&mut self, track: TrackId, item: ItemId, f: impl FnOnce(&mut Clip)) {
        self.edit_track(track, |tr| {
            tr.items.update(&item, |i| f(i.as_clip_mut().unwrap()));
        });
    }

    fn at(&self, time: Time) -> SceneDescription {
        evaluate(&self.project, self.seq, time).unwrap()
    }

    fn items_at(&self, time: Time) -> Vec<ItemId> {
        self.at(time).layers.iter().map(|l| l.item).collect()
    }
}

// ---- table-driven video rules -------------------------------------------------------------

#[test]
fn unknown_sequence_is_the_only_error() {
    let f = Fixture::new();
    let missing = SequenceId::from_u128(999);
    assert_eq!(
        evaluate(&f.project, missing, Time::ZERO),
        Err(EvalError::UnknownSequence(missing))
    );
    assert!(evaluate_audio(&f.project, missing, t(0), t(1)).is_err());
}

#[test]
fn canvas_and_background_come_from_the_sequence() {
    let f = Fixture::new();
    let scene = f.at(sec(5));
    assert_eq!((scene.width, scene.height), (1920, 1080));
    assert_eq!(scene.frame_rate, FrameRate::FPS_24);
    assert_eq!(scene.background, Color::rgb(10, 20, 30));
    assert_eq!(scene.time, sec(5));
    assert!(scene.layers.is_empty() && scene.audio.is_empty());
}

#[test]
fn clip_boundaries_are_start_inclusive_end_exclusive() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let v = f.track(10, TrackKind::Video);
    let a = f.clip(v, m, sec(2), Duration::from_secs(3).unwrap()); // [2s, 5s)
    let table = [
        (sec(0), false),
        (t(2 * S - 1), false),
        (sec(2), true),
        (t(2 * S + 1), true),
        (t(5 * S - 1), true),
        (sec(5), false),
        (sec(100), false),
    ];
    for (time, visible) in table {
        assert_eq!(
            f.items_at(time) == vec![a],
            visible,
            "at {} flicks",
            time.flicks()
        );
    }
}

#[test]
fn gap_between_clips_is_empty_and_abutting_clips_hand_over() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let v = f.track(10, TrackKind::Video);
    let a = f.clip(v, m, sec(0), Duration::from_secs(1).unwrap());
    let b = f.clip(v, m, sec(1), Duration::from_secs(1).unwrap());
    let c = f.clip(v, m, sec(3), Duration::from_secs(1).unwrap());
    assert_eq!(f.items_at(t(S - 1)), vec![a]);
    assert_eq!(f.items_at(sec(1)), vec![b]);
    assert!(f.items_at(sec(2)).is_empty());
    assert_eq!(f.items_at(sec(3)), vec![c]);
}

#[test]
fn source_time_is_source_in_plus_offset() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let v = f.track(10, TrackKind::Video);
    let a = f.clip(v, m, sec(10), Duration::from_secs(5).unwrap());
    f.edit_clip(v, a, |c| c.source_in = sec(20));
    let layer = &f.at(t(12 * S + 7)).layers[0];
    assert_eq!(layer.request.source_time, t(22 * S + 7));
    assert_eq!(layer.request.media, m);
    assert_eq!(layer.request.stream, Some(0));
    assert_eq!(layer.request.source_size, Some((1920, 1080)));
    assert!(!layer.request.still);
    assert_eq!(f.at(sec(10)).layers[0].request.source_time, sec(20));
}

#[test]
fn stacked_tracks_are_ordered_bottom_first() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let low = f.track(10, TrackKind::Video);
    let high = f.track(11, TrackKind::Video); // inserted on top
    let a = f.clip(low, m, sec(0), Duration::from_secs(10).unwrap());
    let b = f.clip(high, m, sec(2), Duration::from_secs(2).unwrap());
    assert_eq!(f.items_at(sec(1)), vec![a]);
    assert_eq!(f.items_at(sec(3)), vec![a, b]);
    assert_eq!(f.at(sec(3)).layers[1].track, high);
    assert_eq!(f.items_at(sec(5)), vec![a]);
}

#[test]
fn hidden_tracks_and_disabled_clips_are_skipped() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let low = f.track(10, TrackKind::Video);
    let high = f.track(11, TrackKind::Video);
    let a = f.clip(low, m, sec(0), Duration::from_secs(10).unwrap());
    let b = f.clip(high, m, sec(0), Duration::from_secs(10).unwrap());
    f.edit_track(high, |tr| tr.hidden = true);
    assert_eq!(f.items_at(sec(1)), vec![a]);
    f.edit_track(high, |tr| tr.hidden = false);
    f.edit_clip(low, a, |c| c.enabled = false);
    assert_eq!(f.items_at(sec(1)), vec![b]);
}

#[test]
fn muting_a_video_track_does_not_hide_it() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let v = f.track(10, TrackKind::Video);
    let a = f.clip(v, m, sec(0), Duration::from_secs(1).unwrap());
    f.edit_track(v, |tr| tr.muted = true);
    assert_eq!(f.items_at(sec(0)), vec![a]);
}

#[test]
fn audio_only_media_and_missing_media_have_no_picture() {
    let mut f = Fixture::new();
    let au = f.media(1, MediaKind::Audio, 60);
    let v = f.track(10, TrackKind::Video);
    f.clip(v, au, sec(0), Duration::from_secs(2).unwrap());
    f.clip(
        v,
        MediaId::from_u128(404),
        sec(2),
        Duration::from_secs(2).unwrap(),
    );
    assert!(f.items_at(sec(1)).is_empty());
    assert!(f.items_at(sec(3)).is_empty());
}

#[test]
fn stills_request_time_zero() {
    let mut f = Fixture::new();
    let img = f.media(1, MediaKind::Image, 0);
    let v = f.track(10, TrackKind::Video);
    f.clip(v, img, sec(4), Duration::from_secs(4).unwrap());
    let layer = &f.at(sec(6)).layers[0];
    assert!(layer.request.still);
    assert_eq!(layer.request.source_time, Time::ZERO);
}

#[test]
fn visual_properties_pass_through_and_are_sanitized() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let v = f.track(10, TrackKind::Video);
    let a = f.clip(v, m, sec(0), Duration::from_secs(1).unwrap());
    f.edit_clip(v, a, |c| {
        c.visual.opacity.set(0.25);
        c.visual.transform.position.set(Vec2::new(0.1, -0.2));
        c.visual.transform.scale.set(Vec2::new(2.0, 0.5));
        c.visual.transform.rotation.set(15.0);
        c.visual.transform.anchor.set(Vec2::new(0.0, 1.0));
    });
    let l = &f.at(Time::ZERO).layers[0];
    assert_eq!(l.opacity, 0.25);
    assert_eq!(l.transform.position, Vec2::new(0.1, -0.2));
    assert_eq!(l.transform.scale, Vec2::new(2.0, 0.5));
    assert_eq!(l.transform.rotation_deg, 15.0);
    assert_eq!(l.transform.anchor, Vec2::new(0.0, 1.0));
    assert!(l.effects.is_empty());

    f.edit_clip(v, a, |c| {
        c.visual.opacity.set(7.0);
        c.visual.transform.rotation.set(f64::NAN);
        c.visual
            .transform
            .position
            .set(Vec2::new(f64::INFINITY, 0.0));
    });
    let l = &f.at(Time::ZERO).layers[0];
    assert_eq!(l.opacity, 1.0);
    assert_eq!(l.transform.rotation_deg, 0.0);
    assert_eq!(l.transform.position, Vec2::new(0.0, 0.0));
}

#[test]
fn crop_flip_and_blend_mode_come_from_the_model() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let v = f.track(10, TrackKind::Video);
    let a = f.clip(v, m, sec(0), Duration::from_secs(1).unwrap());
    let l = &f.at(Time::ZERO).layers[0];
    assert_eq!(l.crop, Crop::default());
    assert_eq!(l.flip, Flip::default());
    assert_eq!(l.blend, BlendMode::Normal);
    f.edit_clip(v, a, |c| {
        c.visual.crop.left.set(0.1);
        c.visual.crop.top.set(0.2);
        c.visual.crop.right.set(0.3);
        c.visual.crop.bottom.set(0.4);
        c.visual.flip.horizontal.set(true);
        c.visual.blend_mode = cutweave_model::BlendMode::SoftLight;
    });
    let l = &f.at(Time::ZERO).layers[0];
    assert_eq!(
        l.crop,
        Crop {
            left: 0.1,
            top: 0.2,
            right: 0.3,
            bottom: 0.4
        }
    );
    assert_eq!(
        l.flip,
        Flip {
            horizontal: true,
            vertical: false
        }
    );
    assert_eq!(l.blend, BlendMode::SoftLight);
    // Out-of-range values (only an invalid document holds them) are clamped, never passed on.
    f.edit_clip(v, a, |c| {
        c.visual.crop.left.set(-0.5);
        c.visual.crop.right.set(3.0);
        c.visual.transform.scale.set(Vec2::new(-1.0, 2.0));
    });
    let l = &f.at(Time::ZERO).layers[0];
    assert_eq!((l.crop.left, l.crop.right), (0.0, 1.0));
    assert_eq!(l.transform.scale, Vec2::new(0.0, 2.0));
}

fn keys<T: cutweave_model::AnimValue>(list: Vec<(i64, T, Interp)>) -> Animatable<T> {
    Animatable::Keyframes(
        Keyframes::new(
            list.into_iter()
                .map(|(at, v, interp)| Keyframe::new(t(at), v, interp))
                .collect(),
        )
        .unwrap(),
    )
}

#[test]
fn animated_properties_are_evaluated_at_item_time_and_clamped() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let v = f.track(10, TrackKind::Video);
    // The clip starts at 10 s and shows its media from 30 s: keys are in item time, so
    // neither moves them.
    let a = f.clip(v, m, sec(10), Duration::from_secs(4).unwrap());
    f.edit_clip(v, a, |c| {
        c.source_in = sec(30);
        c.visual.transform.position = keys(vec![
            (0, Vec2::new(-0.5, 0.0), Interp::Linear),
            (2 * S, Vec2::new(0.5, 0.25), Interp::Linear),
        ]);
        c.visual.transform.rotation = keys(vec![
            (0, 0.0, Interp::Linear),
            (4 * S, 720.0, Interp::Linear),
        ]);
        // An easing that overshoots: opacity is clamped to 1 after interpolation.
        c.visual.opacity = keys(vec![
            (0, 0.0, Easing::BackOut.interp()),
            (2 * S, 1.0, Interp::Linear),
        ]);
        c.visual.crop.left = keys(vec![(0, 0.0, Interp::Linear), (2 * S, 0.5, Interp::Linear)]);
        c.visual.flip.vertical = keys(vec![(0, false, Interp::Hold), (S, true, Interp::Hold)]);
    });
    let at = |time: Time| f.at(time).layers[0].clone();
    let start = at(sec(10));
    assert_eq!(start.transform.position, Vec2::new(-0.5, 0.0));
    assert_eq!(start.opacity, 0.0);
    assert!(!start.flip.vertical);
    let mid = at(t(11 * S));
    assert_eq!(mid.transform.position, Vec2::new(0.0, 0.125));
    assert_eq!(mid.transform.rotation_deg, 180.0);
    assert_eq!(mid.crop.left, 0.25);
    assert!(mid.flip.vertical, "a hold key switches at its time");
    assert_eq!(mid.request.source_time, sec(31));
    // Past the last key the value holds.
    assert_eq!(at(t(13 * S)).transform.position, Vec2::new(0.5, 0.25));
    // The overshooting easing goes above 1 somewhere: never seen.
    let mut overshoot = false;
    for i in 0..=48 {
        let l = at(t(10 * S + i * S / 24));
        assert!((0.0..=1.0).contains(&l.opacity), "{}", l.opacity);
        let raw = f.project.sequences[&f.seq]
            .tracks
            .get(&v)
            .and_then(|tr| tr.items.get(&a))
            .and_then(Item::as_clip)
            .map(|c| c.visual.opacity.eval(t(i * S / 24)))
            .unwrap();
        overshoot |= raw > 1.0;
        if raw > 1.0 {
            assert_eq!(l.opacity, 1.0);
        }
    }
    assert!(overshoot, "the test easing overshoots");
    // The repeated-evaluation index agrees bit for bit.
    let index = TimelineIndex::new(&f.project, f.seq).unwrap();
    for i in 0..=96 {
        let time = t(10 * S + i * S / 24);
        assert_eq!(index.evaluate(time), f.at(time));
    }
}

// ---- audio --------------------------------------------------------------------------------

#[test]
fn audio_comes_from_unmuted_audio_tracks_only() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let vt = f.track(10, TrackKind::Video);
    let at = f.track(11, TrackKind::Audio);
    f.clip(vt, m, sec(0), Duration::from_secs(5).unwrap());
    let a = f.clip(at, m, sec(0), Duration::from_secs(5).unwrap());
    f.edit_clip(at, a, |c| {
        c.source_in = sec(3);
        c.audio.gain.set(-6.0);
    });
    f.edit_track(at, |tr| tr.gain = -3.0);
    let scene = f.at(sec(1));
    assert_eq!(scene.audio.len(), 1);
    let act = &scene.audio[0];
    assert_eq!((act.item, act.track, act.media), (a, at, m));
    assert_eq!(act.stream, Some(1));
    assert_eq!(act.source_time, sec(4));
    assert_eq!((act.clip_gain_db, act.track_gain_db), (-6.0, -3.0));
    assert_eq!(act.speed, (1, 1));
    assert!(!act.reverse);

    f.edit_track(at, |tr| tr.muted = true);
    assert!(f.at(sec(1)).audio.is_empty());
}

#[test]
fn solo_silences_the_other_audio_tracks() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Audio, 60);
    let t1 = f.track(10, TrackKind::Audio);
    let t2 = f.track(11, TrackKind::Audio);
    let a = f.clip(t1, m, sec(0), Duration::from_secs(5).unwrap());
    let b = f.clip(t2, m, sec(0), Duration::from_secs(5).unwrap());
    let ids = |f: &Fixture| {
        f.at(sec(1))
            .audio
            .iter()
            .map(|a| a.item)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&f), vec![b, a]); // track order: top (t2) first
    f.edit_track(t1, |tr| tr.solo = true);
    assert_eq!(ids(&f), vec![a]);
}

#[test]
fn disabled_clips_and_stills_are_silent() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let img = f.media(2, MediaKind::Image, 0);
    let at = f.track(10, TrackKind::Audio);
    let a = f.clip(at, m, sec(0), Duration::from_secs(1).unwrap());
    f.clip(at, img, sec(1), Duration::from_secs(1).unwrap());
    f.edit_clip(at, a, |c| c.enabled = false);
    assert!(f.at(sec(0)).audio.is_empty());
    assert!(f.at(sec(1)).audio.is_empty());
}

#[test]
fn audio_ranges_are_clipped_to_the_range_and_carry_source_offsets() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Audio, 60);
    let at = f.track(10, TrackKind::Audio);
    let a = f.clip(at, m, sec(2), Duration::from_secs(4).unwrap()); // [2s, 6s)
    f.edit_clip(at, a, |c| c.source_in = sec(10));
    let segs = evaluate_audio(&f.project, f.seq, sec(0), sec(4)).unwrap();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].dst_start, sec(2));
    assert_eq!(segs[0].dst_duration, Duration::from_secs(2).unwrap());
    assert_eq!(segs[0].src_start, sec(10));
    assert_eq!(segs[0].sample_rate, SampleRate::HZ_48_000);

    let segs = evaluate_audio(&f.project, f.seq, sec(3), sec(100)).unwrap();
    assert_eq!(segs[0].dst_start, sec(3));
    assert_eq!(segs[0].dst_duration, Duration::from_secs(3).unwrap());
    assert_eq!(segs[0].src_start, sec(11));

    assert!(
        evaluate_audio(&f.project, f.seq, sec(6), sec(9))
            .unwrap()
            .is_empty()
    );
    assert!(
        evaluate_audio(&f.project, f.seq, sec(3), sec(3))
            .unwrap()
            .is_empty()
    );
}

// ---- frame-boundary edge cases ------------------------------------------------------------

#[test]
fn back_to_back_frame_clips_hand_over_exactly_on_frame_boundaries() {
    for rate in FrameRate::COMMON {
        let mut f = Fixture::new();
        let m = f.media(1, MediaKind::Video, 600);
        let v = f.track(10, TrackKind::Video);
        let frames = [0_i64, 1, 29, 30, 31, 1000, 1001, 1002];
        let mut ids = Vec::new();
        for w in frames.windows(2) {
            let start = rate.frame_to_time(w[0]).unwrap();
            let end = rate.frame_to_time(w[1]).unwrap();
            ids.push(f.clip(v, m, start, end.duration_since(start).unwrap()));
        }
        for (i, w) in frames.windows(2).enumerate() {
            let start = rate.frame_to_time(w[0]).unwrap();
            let end = rate.frame_to_time(w[1]).unwrap();
            assert_eq!(
                f.items_at(start),
                vec![ids[i]],
                "{rate:?} start of clip {i}"
            );
            assert_eq!(
                f.items_at(t(end.flicks() - 1)),
                vec![ids[i]],
                "{rate:?} last flick of clip {i}"
            );
            if i + 1 < ids.len() {
                assert_eq!(f.items_at(end), vec![ids[i + 1]], "{rate:?} hand-over {i}");
            } else {
                assert!(f.items_at(end).is_empty(), "{rate:?} end of last clip");
            }
        }
    }
}

#[test]
fn source_time_of_a_frame_start_is_a_frame_boundary_at_ntsc_rates() {
    let rate = FrameRate::FPS_29_97;
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 600);
    let v = f.track(10, TrackKind::Video);
    let start = rate.frame_to_time(100).unwrap();
    let a = f.clip(
        v,
        m,
        start,
        rate.frame_to_time(50)
            .unwrap()
            .duration_since(Time::ZERO)
            .unwrap(),
    );
    f.edit_clip(v, a, |c| c.source_in = rate.frame_to_time(1000).unwrap());
    for n in [0_i64, 1, 17, 49] {
        let time = rate.frame_to_time(100 + n).unwrap();
        let src = f.at(time).layers[0].request.source_time;
        assert_eq!(src, rate.frame_to_time(1000 + n).unwrap(), "frame {n}");
        assert_eq!(
            rate.time_to_frame(src, cutweave_model::Rounding::Floor),
            1000 + n
        );
    }
}

// ---- determinism and scale ----------------------------------------------------------------

#[test]
fn evaluation_is_repeatable() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 60);
    let v1 = f.track(10, TrackKind::Video);
    let v2 = f.track(11, TrackKind::Video);
    let at = f.track(12, TrackKind::Audio);
    for (tr, s) in [(v1, 0), (v2, 1), (at, 0)] {
        f.clip(tr, m, sec(s), Duration::from_secs(5).unwrap());
    }
    assert_eq!(f.at(sec(2)), f.at(sec(2)));
    let cloned = f.project.clone();
    assert_eq!(evaluate(&cloned, f.seq, sec(2)).unwrap(), f.at(sec(2)));
}

#[test]
fn five_thousand_items_evaluate_quickly() {
    let mut f = Fixture::new();
    let m = f.media(1, MediaKind::Video, 100_000);
    let tracks: Vec<_> = (0..5).map(|i| f.track(10 + i, TrackKind::Video)).collect();
    for (k, tr) in tracks.iter().enumerate() {
        for i in 0..1000_i64 {
            f.clip(
                *tr,
                m,
                sec(i * 2 + k as i64),
                Duration::from_secs(1).unwrap(),
            );
        }
    }
    assert_eq!(f.project.item_count(), 5000);
    let start = std::time::Instant::now();
    let mut visible = 0;
    for i in 0..100 {
        visible += f.at(t(1000 * S + i)).layers.len();
    }
    let per_eval = start.elapsed() / 100;
    eprintln!("5000-item evaluate: {per_eval:?} per call ({visible} layers in 100 calls)");
    // The index (multi-node trees here) agrees, on and between clips.
    let index = TimelineIndex::new(&f.project, f.seq).unwrap();
    for flicks in [
        0,
        S / 2,
        S,
        1000 * S + 17,
        1999 * S,
        2003 * S,
        2004 * S,
        10_000 * S,
    ] {
        assert_eq!(index.evaluate(t(flicks)), f.at(t(flicks)), "at {flicks}");
    }
    assert!(
        per_eval < std::time::Duration::from_millis(5),
        "{per_eval:?}"
    );
}

// ---- properties ---------------------------------------------------------------------------

fn project_and_times() -> impl Strategy<Value = (Project, Vec<i64>)> {
    cutweave_model::testing::arb_project().prop_flat_map(|p| {
        let end = p
            .sequences
            .values()
            .map(|s| s.duration().flicks())
            .max()
            .unwrap_or(0)
            + 2 * S;
        (Just(p), prop::collection::vec(-S..end, 1..12))
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn evaluation_never_panics_and_is_deterministic((project, times) in project_and_times()) {
        for seq in project.sequences.keys() {
            for &time in &times {
                let time = Time::from_flicks(time).unwrap();
                let a = evaluate(&project, *seq, time).unwrap();
                let b = evaluate(&project.clone(), *seq, time).unwrap();
                prop_assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn the_timeline_index_evaluates_like_evaluate((project, times) in project_and_times()) {
        for seq in project.sequences.keys() {
            let index = TimelineIndex::new(&project, *seq).unwrap();
            prop_assert_eq!(index.sequence(), *seq);
            // One reused scene across times, as the preview keeps it.
            let mut reused = index.evaluate(Time::ZERO);
            for &time in &times {
                let time = Time::from_flicks(time).unwrap();
                let expected = evaluate(&project, *seq, time).unwrap();
                index.evaluate_into(time, &mut reused);
                prop_assert_eq!(&reused, &expected);
                prop_assert_eq!(index.evaluate(time), expected);
            }
        }
    }

    #[test]
    fn layers_are_active_in_bounds_and_sane((project, times) in project_and_times()) {
        for sequence in project.sequences.values() {
            for &time in &times {
                let time = Time::from_flicks(time).unwrap();
                let scene = evaluate(&project, sequence.id, time).unwrap();
                for layer in &scene.layers {
                    let (track, item) = sequence.find_item(&layer.item).unwrap();
                    prop_assert_eq!(track.id, layer.track);
                    prop_assert!(item.enabled() && !track.hidden);
                    prop_assert!(item.start() <= time && time < item.end().unwrap());
                    let media = &project.media[&layer.request.media];
                    prop_assert!(layer.request.source_time >= Time::ZERO);
                    if !layer.request.still && let Some(dur) = media.probe.duration {
                        prop_assert!(
                            layer.request.source_time.flicks() < dur.flicks().max(1),
                            "source {} beyond media {}", layer.request.source_time.flicks(), dur.flicks()
                        );
                    }
                    prop_assert!((0.0..=1.0).contains(&layer.opacity));
                }
                // One layer per video track at most, bottom first.
                let mut order: Vec<_> = sequence.tracks_in_order().map(|t| t.id).collect();
                order.reverse();
                let positions: Vec<_> = scene
                    .layers
                    .iter()
                    .map(|l| order.iter().position(|t| *t == l.track).unwrap())
                    .collect();
                prop_assert!(positions.windows(2).all(|w| w[0] < w[1]));
                for act in &scene.audio {
                    let (track, item) = sequence.find_item(&act.item).unwrap();
                    prop_assert!(track.kind == TrackKind::Audio && !track.muted && item.enabled());
                }
            }
        }
    }

    #[test]
    fn audio_activations_agree_with_audio_segments((project, times) in project_and_times()) {
        for seq in project.sequences.keys() {
            for &time in &times {
                let time = Time::from_flicks(time).unwrap();
                let scene = evaluate(&project, *seq, time).unwrap();
                let end = Time::from_flicks(time.flicks() + 1).unwrap();
                let mut segs = evaluate_audio(&project, *seq, time, end).unwrap();
                let mut acts: Vec<_> = scene.audio.iter().map(|a| (a.item, a.source_time)).collect();
                let mut from_segs: Vec<_> = segs.drain(..).map(|s| (s.item, s.src_start)).collect();
                acts.sort_by_key(|a| a.0);
                from_segs.sort_by_key(|a| a.0);
                prop_assert_eq!(acts, from_segs);
            }
        }
    }
}

// ---- snapshots ----------------------------------------------------------------------------

#[test]
fn snapshot_stacked_pip_with_audio() {
    let mut f = Fixture::new();
    let video = f.media(1, MediaKind::Video, 120);
    let image = f.media(2, MediaKind::Image, 0);
    let music = f.media(3, MediaKind::Audio, 300);
    let top = f.track(10, TrackKind::Video);
    let base = f.track(11, TrackKind::Video);
    let a1 = f.track(12, TrackKind::Audio);
    let main = f.clip(base, video, sec(0), Duration::from_secs(10).unwrap());
    f.edit_clip(base, main, |c| c.source_in = sec(30));
    let logo = f.clip(top, image, sec(2), Duration::from_secs(4).unwrap());
    f.edit_clip(top, logo, |c| {
        c.visual.opacity.set(0.5);
        c.visual.transform.position.set(Vec2::new(0.35, -0.3));
        c.visual.transform.scale.set(Vec2::new(0.25, 0.25));
    });
    let bed = f.clip(a1, music, sec(0), Duration::from_secs(10).unwrap());
    f.edit_clip(a1, bed, |c| c.audio.gain.set(-12.0));
    insta::assert_json_snapshot!("pip_with_audio_at_3s", f.at(sec(3)));
}

#[test]
fn snapshot_hidden_track_disabled_clip_and_gap() {
    let mut f = Fixture::new();
    let video = f.media(1, MediaKind::Video, 120);
    let hidden = f.track(10, TrackKind::Video);
    let shown = f.track(11, TrackKind::Video);
    f.clip(hidden, video, sec(0), Duration::from_secs(5).unwrap());
    let off = f.clip(shown, video, sec(0), Duration::from_secs(2).unwrap());
    f.clip(shown, video, sec(3), Duration::from_secs(2).unwrap());
    f.edit_track(hidden, |tr| tr.hidden = true);
    f.edit_clip(shown, off, |c| c.enabled = false);
    insta::assert_json_snapshot!(
        "hidden_disabled_gap",
        [f.at(sec(1)), f.at(t(5 * S / 2)), f.at(sec(4))]
    );
}

#[test]
fn snapshot_ntsc_frame_boundary() {
    let rate = FrameRate::FPS_29_97;
    let mut f = Fixture::new();
    f.project
        .sequences
        .get_mut(&f.seq)
        .unwrap()
        .format
        .frame_rate = rate;
    let video = f.media(1, MediaKind::Video, 120);
    let v = f.track(10, TrackKind::Video);
    let boundary = rate.frame_to_time(30).unwrap();
    f.clip(
        v,
        video,
        Time::ZERO,
        boundary.duration_since(Time::ZERO).unwrap(),
    );
    let second = f.clip(v, video, boundary, Duration::from_secs(1).unwrap());
    f.edit_clip(v, second, |c| c.source_in = sec(7));
    insta::assert_json_snapshot!(
        "ntsc_boundary",
        [f.at(t(boundary.flicks() - 1)), f.at(boundary)]
    );
}
