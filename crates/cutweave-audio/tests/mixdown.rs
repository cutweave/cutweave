// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline mixdown acceptance tests (WP-1G.01): the analytical sum of overlapping tones,
//! sample-accurate clip boundaries, and the document → `evaluate_audio` → mix path.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod common;

use std::sync::Arc;

use common::{RATE, Seg, factory, offline, sample_of, secs, segments, sine};
use cutweave_audio::{
    MIXDOWN_CHUNK_FRAMES, PcmFormat, SourceFactory, Tone, ToneFactory, ToneSpec, db_to_gain,
    mixdown,
};
use cutweave_model::{
    Clip, Duration, Extra, FrameRate, ItemId, MediaAsset, MediaId, MediaKind, ProbeInfo, Project,
    Rounding, SampleRate, Sequence, SequenceFormat, SequenceId, StreamInfo, StreamKind, Time,
    Track, TrackId, TrackKind,
};

/// Four tone clips on four tracks, overlapping in time, with different rates of channel
/// layout, source offsets, clip and track gains, and boundaries off the sample grid.
#[test]
fn four_overlapping_tones_equal_the_analytical_sum() {
    let specs = [
        sine(440.0, 0.2, 0.0, RATE, 1),
        sine(1000.0, 0.15, 1.0, RATE, 2),
        sine(3150.5, 0.1, -0.5, RATE, 1),
        sine(60.0, 0.05, 0.3, RATE, 6), // 5.1: every channel carries the tone
    ];
    let sources = factory(&specs);
    let fps = FrameRate::FPS_29_97;
    let frame = |n| fps.frame_to_time(n).unwrap();
    let segs = [
        Seg::new(1, 1, 1, frame(3), frame(40))
            .src(secs(0.5))
            .gains(-3.0, 0.0),
        Seg::new(2, 2, 2, frame(8), frame(60)).gains(0.0, -6.0),
        Seg::new(3, 3, 3, frame(15), frame(52))
            .src(secs(2.0))
            .gains(2.0, -1.0),
        Seg::new(4, 4, 4, Time::ZERO, frame(66)).src(frame(1)),
    ];
    let end = frame(70);
    let mix = offline(&sources, &segments(&segs), Time::ZERO, end);
    assert_eq!(mix.len() as i64, sample_of(end) * 2);

    // 5.1 → stereo: L = FL + C/√2 + Ls/√2 with all three equal.
    // Mono sources are panned centre with the constant-power law (−3 dB per channel).
    let half = std::f64::consts::FRAC_1_SQRT_2;
    let downmix = [half, 1.0, half, 1.0 + std::f64::consts::SQRT_2];
    let mut worst = 0.0f64;
    let mut overlap_samples = 0;
    for (n, frame) in mix.as_chunks::<2>().0.iter().enumerate() {
        let n = n as i64;
        let mut want = 0.0f64;
        let mut active = 0;
        for (k, s) in segs.iter().enumerate() {
            let (a, b) = (sample_of(s.start), sample_of(s.end()));
            if n < a || n >= b {
                continue;
            }
            active += 1;
            let src0 = specs[k]
                .format
                .rate
                .time_to_samples(s.src_start, Rounding::Nearest);
            let value = f64::from(specs[k].sample((src0 + n - a) as u64));
            want += value
                * downmix[k]
                * f64::from(db_to_gain(s.clip_db))
                * f64::from(db_to_gain(s.track_db));
        }
        if active == 4 {
            overlap_samples += 1;
        }
        for &got in frame {
            worst = worst.max((f64::from(got) - want).abs());
        }
    }
    assert!(overlap_samples > 10_000, "the four clips overlap");
    assert!(worst < 1e-5, "max error {worst:e} vs the analytical sum");
    eprintln!(
        "4-tone mix: max |mix − analytical| = {worst:.2e} over {} frames",
        mix.len() / 2
    );
}

/// Impulse at a clip's first source frame lands exactly on `time_to_samples(start)`; DC clips
/// fill exactly `[time_to_samples(start), time_to_samples(end))`, abutting with no gap.
#[test]
fn clip_boundaries_are_sample_accurate() {
    let stereo = PcmFormat::new(RATE, 2);
    // Below the limiter knee, so every value passes the master stage unchanged.
    let impulse = ToneSpec::new(
        Tone::Impulse {
            amplitude: 0.75,
            period: 0,
        },
        stereo,
    );
    let train = ToneSpec::new(
        Tone::Impulse {
            amplitude: 0.5,
            period: 480,
        },
        stereo,
    );
    let dc = |amplitude| {
        ToneSpec::new(
            Tone::Impulse {
                amplitude,
                period: 1,
            },
            stereo,
        )
    };
    let sources = factory(&[impulse, train, dc(0.25), dc(0.125)]);

    let fps = FrameRate::FPS_29_97; // frame boundaries fall between samples
    let frame = |n| fps.frame_to_time(n).unwrap();
    let segs = [
        Seg::new(1, 1, 1, frame(17), frame(30)),
        // Source starts on the 3rd impulse of the train: impulses at the clip start, then every
        // 480 samples.
        Seg::new(2, 2, 2, frame(5), frame(9)).src(RATE.samples_to_time(960).unwrap()),
        Seg::new(3, 3, 3, frame(10), frame(12)),
        Seg::new(3, 4, 4, frame(12), frame(15)),
    ];
    let mix = offline(&sources, &segments(&segs), Time::ZERO, frame(40));
    let left: Vec<f32> = mix.iter().step_by(2).copied().collect();
    let at = |t: Time| sample_of(t) as usize;

    // Clip 1: the single impulse.
    let first = at(frame(17));
    assert_eq!(
        first as i64,
        RATE.time_to_samples(frame(17), Rounding::Nearest)
    );
    assert_eq!(left[first], 0.75);
    // Clip 2: impulses at its start and every 480 samples, nowhere else in its range.
    let (a, b) = (at(frame(5)), at(frame(9)));
    for (n, &v) in left.iter().enumerate().take(b).skip(a) {
        let want = if (n - a) % 480 == 0 { 0.5 } else { 0.0 };
        assert_eq!(v, want, "train sample {n}");
    }
    // Clips 3 and 4 abut: DC exactly over their sample ranges.
    let (c3, c4, c5) = (at(frame(10)), at(frame(12)), at(frame(15)));
    assert_eq!(left[c3 - 1], 0.0);
    assert!(left[c3..c4].iter().all(|&v| v == 0.25));
    assert!(left[c4..c5].iter().all(|&v| v == 0.125));
    assert_eq!(left[c5], 0.0);
    // Nothing else is audible.
    let audible = left.iter().filter(|&&v| v != 0.0).count();
    let train_hits = (b - a).div_ceil(480);
    assert_eq!(audible, 1 + train_hits + (c5 - c3));
    // Left and right are identical for centred sources.
    for f in mix.as_chunks::<2>().0 {
        assert_eq!(f[0], f[1]);
    }
}

/// Chunks tile the range exactly; the iterator is lazy and reports its length.
#[test]
fn mixdown_chunks_cover_the_range() {
    let sources = factory(&[sine(440.0, 0.5, 0.0, RATE, 1)]);
    let segs = segments(&[Seg::new(1, 1, 1, secs(0.0), secs(1.0))]);
    let range = secs(0.25)..secs(0.5);
    let m = mixdown(&sources, &segs, range.clone(), RATE).unwrap();
    assert_eq!(m.samples(), 12_000..24_000);
    assert_eq!(m.len(), 12_000usize.div_ceil(MIXDOWN_CHUNK_FRAMES));
    let mut next = 12_000;
    for chunk in m {
        assert_eq!(chunk.start, next);
        assert_eq!(chunk.time().unwrap(), RATE.samples_to_time(next).unwrap());
        next += chunk.frames() as i64;
    }
    assert_eq!(next, 24_000);
}

/// A project with audio tracks goes through `evaluate_audio` and mixes: track gain, clip gain,
/// mute and solo all apply.
#[test]
fn project_mixes_through_evaluate_audio() {
    let seq = SequenceId::from_u128(1);
    let mut project = Project::new("mix");
    let mut sequence = Sequence::new(
        seq,
        "Main",
        SequenceFormat::new(1280, 720, FrameRate::FPS_30, SampleRate::HZ_48_000),
    );
    let tracks = [
        TrackId::from_u128(11),
        TrackId::from_u128(12),
        TrackId::from_u128(13),
    ];
    for (i, t) in tracks.iter().enumerate() {
        sequence.insert_track(i, Track::new(*t, TrackKind::Audio, format!("A{i}")));
    }
    let mut tones = ToneFactory::new();
    for (i, track) in tracks.iter().enumerate() {
        let media = MediaId::from_u128(21 + i as u128);
        let mut probe = ProbeInfo::default();
        probe.streams.push(StreamInfo {
            index: 0,
            kind: StreamKind::Audio,
            codec: None,
            profile: None,
            width: None,
            height: None,
            pixel_format: None,
            sample_rate: Some(SampleRate::HZ_48_000),
            channels: Some(2),
            language: None,
            extra: Extra::new(),
        });
        probe.duration = Some(Duration::from_secs(60).unwrap());
        project.media.insert(
            media,
            MediaAsset::file(media, MediaKind::Audio, format!("/m/{i}.wav"), probe),
        );
        tones.insert(
            media,
            ToneSpec::new(
                Tone::Impulse {
                    amplitude: 0.25,
                    period: 1,
                },
                PcmFormat::new(SampleRate::HZ_48_000, 2),
            ),
        );
        let mut clip = Clip::new(
            ItemId::from_u128(31 + i as u128),
            media,
            secs(i as f64 * 0.5),
            Duration::from_secs(1).unwrap(),
        );
        clip.audio.gain = cutweave_model::Animatable::Static(-6.0);
        let tr = sequence.track_mut(track).unwrap();
        tr.gain = -6.0 * i as f64;
        tr.items.insert(clip.into());
    }
    sequence.track_mut(&tracks[2]).unwrap().muted = true;
    project.sequences.insert(seq, sequence);
    let sources: Arc<dyn SourceFactory> = Arc::new(tones);

    let segs = cutweave_render::evaluate_audio(&project, seq, Time::ZERO, secs(3.0)).unwrap();
    assert_eq!(segs.len(), 2, "the muted track is not mixed");
    let mix = offline(&sources, &segs, Time::ZERO, secs(3.0));
    let left = |s: f64| mix[sample_of(secs(s)) as usize * 2];
    let g = |db| f64::from(db_to_gain(db));
    let a = 0.25 * g(-6.0);
    let b = 0.25 * g(-6.0) * g(-6.0);
    assert!((f64::from(left(0.25)) - a).abs() < 1e-6);
    assert!((f64::from(left(0.75)) - (a + b)).abs() < 1e-6);
    assert!((f64::from(left(1.25)) - b).abs() < 1e-6);
    assert_eq!(left(1.75), 0.0);

    // Solo the second track: only it plays.
    project
        .sequences
        .get_mut(&seq)
        .unwrap()
        .track_mut(&tracks[1])
        .unwrap()
        .solo = true;
    let segs = cutweave_render::evaluate_audio(&project, seq, Time::ZERO, secs(3.0)).unwrap();
    let mix = offline(&sources, &segs, Time::ZERO, secs(3.0));
    assert_eq!(mix[sample_of(secs(0.25)) as usize * 2], 0.0);
    assert!((f64::from(mix[sample_of(secs(0.75)) as usize * 2]) - b).abs() < 1e-6);
}

/// The master limiter keeps a hot mix (four 0.9 sines in phase: +11 dBFS) under its −1 dBTP
/// ceiling (WP-2E.02; the WP-1G.01 soft clipper stopped at 0 dBFS), without crushing it.
#[test]
fn hot_mixes_stay_below_full_scale() {
    let sources = factory(&[sine(100.0, 0.9, 0.0, RATE, 2)]);
    let segs: Vec<Seg> = (1..=4)
        .map(|t| Seg::new(t, t, 1, Time::ZERO, secs(0.2)))
        .collect();
    let mix = offline(&sources, &segments(&segs), Time::ZERO, secs(0.2));
    let peak = mix.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let ceiling = 10f32.powf(-1.0 / 20.0);
    assert!(peak <= ceiling && peak > 0.85, "peak {peak}");
}

#[test]
fn unsupported_segments_are_rejected() {
    let sources: Arc<dyn SourceFactory> = Arc::new(ToneFactory::new());
    let mut seg = Seg::new(1, 1, 1, Time::ZERO, secs(1.0)).segment();
    seg.speed = (1, 20);
    assert!(mixdown(&sources, &[seg], Time::ZERO..secs(1.0), RATE).is_err());
}

/// Zero crossings per second of the left channel of `mix` (interleaved stereo at 48 kHz).
fn crossings(mix: &[f32]) -> usize {
    let left: Vec<f32> = mix.iter().step_by(2).copied().collect();
    left.windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count()
}

/// WP-2D.01: a clip at 2x plays its source twice as fast (pitch follows until WP-2D.02), at 0.5x
/// half as fast; the offline mix is deterministic.
#[test]
fn speed_changes_play_the_source_faster_or_slower() {
    let sources = factory(&[sine(100.0, 0.5, 0.0, RATE, 2)]);
    let mut seg = Seg::new(1, 1, 1, Time::ZERO, secs(1.0)).segment();
    let normal = crossings(&offline(&sources, &[seg.clone()], Time::ZERO, secs(1.0)));
    seg.speed = (2, 1);
    let fast = offline(&sources, &[seg.clone()], Time::ZERO, secs(1.0));
    assert_eq!(
        fast,
        offline(&sources, &[seg.clone()], Time::ZERO, secs(1.0))
    );
    seg.speed = (1, 2);
    let slow = crossings(&offline(&sources, &[seg], Time::ZERO, secs(1.0)));
    let fast = crossings(&fast);
    assert!((199..=201).contains(&normal), "{normal}");
    assert!((396..=404).contains(&fast), "{fast}");
    assert!((98..=102).contains(&slow), "{slow}");
}

/// WP-2D.01: a reversed clip plays its source range backwards, sample for sample: output sample
/// `k` of a clip whose range ends at source frame 24,000 is source frame 23,999 − k.
#[test]
fn a_reversed_clip_plays_its_source_backwards() {
    let spec = ToneSpec::new(
        Tone::Impulse {
            amplitude: 0.5,
            period: 1000,
        },
        PcmFormat::new(RATE, 1),
    );
    let sources = factory(&[spec]);
    let mut seg = Seg::new(1, 1, 1, Time::ZERO, secs(0.5)).segment();
    // The model's mapping: the first position is one flick before the end of [0, 0.5 s).
    seg.src_start = Time::from_flicks(secs(0.5).flicks() - 1).unwrap();
    seg.reverse = true;
    let mix = offline(&sources, &[seg], Time::ZERO, secs(0.5));
    let hits: Vec<usize> = mix
        .iter()
        .step_by(2)
        .enumerate()
        .filter(|(_, s)| s.abs() > 0.1)
        .map(|(k, _)| k)
        .collect();
    let want: Vec<usize> = (0..24).map(|i| 999 + 1000 * i).collect();
    assert_eq!(hits, want);
}
