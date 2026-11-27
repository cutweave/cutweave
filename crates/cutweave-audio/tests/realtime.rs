// SPDX-License-Identifier: GPL-3.0-or-later
//! Real-time mixer tests (WP-1G.01, WP-2E.02): a paced mock sink with 8 sources and 256-frame
//! buffers (0 underruns, 0 heap operations in the callback, meters included; 120 s with the
//! limiter working), offline == real-time, seeks, edits during playback and gain changes.
//!
//! The counting allocator stands in for `assert_no_alloc` (BSD-1-Clause, not allowed by
//! `deny.toml`): every heap operation inside an `RtSection` on the callback thread is counted.
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
use std::time::{Duration as StdDuration, Instant};

use common::{
    RATE, Seg, eight_sources, factory, max_diff, offline, render_ready, sample_of, secs, segments,
    sine,
};
use cutweave_audio::rt_alloc::{CountingAlloc, RtSection};
use cutweave_audio::{
    BusGraph, BusLevel, BusParams, LoudnessMeter, MAX_METER_BUSES, METER_HZ, Mixer, MixerConfig,
    MixerHandle, PcmFormat, SourceFactory, Tone, ToneSpec, db_to_gain,
};
use cutweave_model::{FrameRate, SampleRate, Time};
use proptest::prelude::*;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn mixer(sources: &Arc<dyn SourceFactory>) -> (Mixer, MixerHandle) {
    Mixer::new(MixerConfig::new(RATE), sources.clone()).unwrap()
}

struct SinkReport {
    callbacks: u64,
    frames: u64,
    heap_ops: u64,
    max_callback: StdDuration,
    late_wakeups: u64,
    /// The smallest limiter gain the meters saw.
    min_limiter_gain: f32,
    /// The last master loudness reading (integrated, LUFS).
    integrated_lufs: f64,
}

/// A mock device: calls `render` with `buffer`-frame buffers on the device's schedule
/// (absolute deadlines every `buffer / rate`), for `seconds` of advancing output. Like the
/// output callback it also meters inside the real-time section: the master loudness every
/// callback, the bus levels and the limiter gain every meter window.
fn paced_sink(mixer: &mut Mixer, buffer: usize, seconds: f64) -> SinkReport {
    let mut out = vec![0.0f32; buffer * 2];
    let period = StdDuration::from_secs_f64(buffer as f64 / f64::from(RATE.hz()));
    let total = (seconds * f64::from(RATE.hz())) as u64;
    let mut loudness = LoudnessMeter::new(RATE).unwrap();
    let mut buses = [BusLevel::SILENT; MAX_METER_BUSES];
    let window = u64::from(RATE.hz() / METER_HZ);
    let mut metered = 0u64;
    let mut report = SinkReport {
        callbacks: 0,
        frames: 0,
        heap_ops: 0,
        max_callback: StdDuration::ZERO,
        late_wakeups: 0,
        min_limiter_gain: 1.0,
        integrated_lufs: f64::NEG_INFINITY,
    };
    let mut deadline = Instant::now();
    while report.frames < total {
        let t0 = Instant::now();
        let section = RtSection::enter();
        let info = mixer.render(&mut out);
        if info.position.is_some() {
            loudness.add(&out);
            metered += buffer as u64;
            if metered >= window {
                metered -= window;
                let _ = mixer.take_bus_levels(&mut buses);
                report.min_limiter_gain = report.min_limiter_gain.min(mixer.take_limiter_gain());
                report.integrated_lufs = loudness.reading().integrated;
                let _ = loudness.take_true_peak();
            }
        }
        report.heap_ops += section.exit();
        report.max_callback = report.max_callback.max(t0.elapsed());
        report.callbacks += 1;
        if info.position.is_some() {
            report.frames += buffer as u64;
        }
        deadline += period;
        let now = Instant::now();
        if now < deadline {
            std::thread::sleep(deadline - now);
        } else if now - deadline > period {
            report.late_wakeups += 1;
        }
    }
    report
}

/// Runs the paced sink over 8 tracks for `seconds`. `hot`: every bus at +20 dB, so the master
/// limiter works all the time (WBS 2E.02 "active limiter").
fn soak(seconds: f64, hot: bool) {
    let (sources, segs) = eight_sources(seconds);
    let segs = segments(&segs);
    let mut graph = BusGraph::from_segments(&segs);
    if hot {
        for bus in graph.buses().to_vec() {
            graph.set(BusParams {
                gain_db: bus.gain_db + 20.0,
                ..bus
            });
        }
    }
    let (mut mixer, handle) = mixer(&sources);
    handle.set_mix(&segs, &graph).unwrap();
    handle.seek(Time::ZERO);
    let report = paced_sink(&mut mixer, 256, seconds);
    let stats = handle.stats();
    let lookahead_ms = stats
        .min_lookahead_frames
        .map_or(f64::NAN, |f| f as f64 * 1000.0 / f64::from(RATE.hz()));
    eprintln!(
        "soak {seconds} s, 8 sources, 256-frame buffers, hot {hot}: {} callbacks, {} underrun \
         frames ({} callbacks), {} RT heap ops, max callback {:?}, min lane look-ahead \
         {lookahead_ms:.1} ms, {} late sink wake-ups, {} source errors, {} leaked plans, \
         min limiter gain {:.3}, integrated {:.1} LUFS",
        report.callbacks,
        stats.underrun_frames,
        stats.underrun_callbacks,
        report.heap_ops,
        report.max_callback,
        report.late_wakeups,
        stats.source_errors,
        stats.leaked_plans,
        report.min_limiter_gain,
        report.integrated_lufs,
    );
    assert_eq!(report.heap_ops, 0, "the callback touched the heap");
    if hot {
        assert!(report.min_limiter_gain < 0.5, "the limiter was not working");
    } else {
        assert_eq!(report.min_limiter_gain, 1.0);
    }
    assert!(report.integrated_lufs.is_finite());
    assert_eq!(stats.underrun_frames, 0, "underruns");
    assert_eq!(stats.source_errors, 0);
    assert_eq!(stats.leaked_plans, 0);
    assert_eq!(stats.position, sample_of(Time::ZERO) + report.frames as i64);
}

/// WBS acceptance: simulated real-time run, mock sink, 256-frame buffers, 8 sources:
/// 0 underruns, 0 callback allocations. Short variant for `just check`.
#[test]
fn realtime_8_sources_256_frames_no_underruns_no_allocations() {
    soak(3.0, false);
}

/// WBS 2E.02 acceptance: 120 s at 256-frame buffers with 8 tracks and the limiter working on
/// every buffer: 0 underruns, 0 heap operations in the callback (meters included). Paced in
/// real time, so it is in the nextest retry filter for loaded machines.
#[test]
fn realtime_soak_120_s_with_the_limiter_active() {
    soak(120.0, true);
}

/// The 10-minute variant (nightly): `cargo nextest run -p cutweave-audio --run-ignored only`.
/// `CUTWEAVE_AUDIO_SOAK_SECS` overrides the duration.
#[test]
#[ignore = "10-minute real-time soak; nightly"]
fn realtime_soak_10_minutes() {
    let seconds = std::env::var("CUTWEAVE_AUDIO_SOAK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600.0);
    soak(seconds, true);
}

/// Plan application (seek, new lanes, hand-offs) inside `render` does not allocate either.
#[test]
fn applying_plans_in_the_callback_does_not_allocate() {
    let (sources, segs) = eight_sources(2.0);
    let (mut mixer, handle) = mixer(&sources);
    let mut out = vec![0.0f32; 512];
    let mut heap = 0;
    let segs_a = segments(&segs);
    let mut segs_b = segs.clone();
    for s in &mut segs_b {
        s.clip_db -= 3.0; // every lane changes: hand-offs on all 8
    }
    let segs_b = segments(&segs_b);
    handle.set_segments(&segs_a).unwrap();
    handle.seek(secs(0.5));
    for round in 0..40 {
        if round == 10 {
            handle.set_segments(&segs_b).unwrap();
        }
        if round == 20 {
            handle.seek(secs(1.0));
        }
        if round == 30 {
            handle.set_master_gain(-6.0);
        }
        // `ready` applies queued plans too: measure it as well.
        loop {
            let s = RtSection::enter();
            let ready = mixer.ready(4800);
            heap += s.exit();
            if ready {
                break;
            }
            std::thread::sleep(StdDuration::from_micros(200));
        }
        let s = RtSection::enter();
        mixer.render(&mut out);
        heap += s.exit();
    }
    assert_eq!(heap, 0);
    let stats = handle.stats();
    assert_eq!(stats.plans_applied, stats.plans_sent);
}

/// Real-time output with a seek in the middle equals the offline mixdown of both ranges.
#[test]
fn seek_flushes_and_refills() {
    let (sources, segs) = eight_sources(3.0);
    let segs = segments(&segs);
    let (mut mixer, handle) = mixer(&sources);
    handle.set_segments(&segs).unwrap();
    handle.seek(secs(0.2));
    let a = render_ready(&mut mixer, 24_064, 256);
    let want = offline(
        &sources,
        &segs,
        secs(0.2),
        RATE.samples_to_time(9_600 + 24_064).unwrap(),
    );
    let (diff, at) = max_diff(&a, &want);
    assert!(diff <= 1e-6, "before seek: diff {diff} at {at}");
    // Seek back into the middle of clips (resampled sources included).
    handle.seek(secs(0.1));
    let _ = mixer.ready(0); // applies the seek
    assert_eq!(mixer.position(), sample_of(secs(0.1)));
    let b = render_ready(&mut mixer, 48_000, 480);
    let (diff, at) = max_diff(&b, &offline(&sources, &segs, secs(0.1), secs(1.1)));
    assert!(diff <= 1e-6, "after seek: diff {diff} at {at}");
    assert_eq!(handle.stats().underrun_frames, 0);
}

/// An edit during playback: unchanged lanes keep playing, changed and new lanes switch exactly
/// `edit_latency` after the playhead, removed lanes go silent there.
#[test]
fn edits_during_playback_switch_at_their_sample() {
    let specs = [
        sine(300.0, 0.1, 0.0, RATE, 1),
        sine(500.0, 0.1, 0.0, RATE, 2),
        sine(700.0, 0.1, 0.0, RATE, 1),
        sine(900.0, 0.1, 0.0, RATE, 1),
        sine(1100.0, 0.1, 0.0, RATE, 1),
    ];
    let sources = factory(&specs);
    let before = segments(&[
        Seg::new(1, 1, 1, secs(0.0), secs(2.0)),
        Seg::new(2, 2, 2, secs(0.0), secs(2.0)),
        Seg::new(4, 4, 4, secs(0.0), secs(2.0)),
    ]);
    let after = segments(&[
        Seg::new(1, 1, 1, secs(0.0), secs(2.0)),
        Seg::new(2, 2, 3, secs(0.0), secs(2.0)), // track 2 now plays another media
        Seg::new(3, 3, 5, secs(0.0), secs(2.0)), // new track
                                                 // track 4 removed
    ]);
    let (mut mixer, handle) = mixer(&sources);
    handle.set_segments(&before).unwrap();
    handle.seek(Time::ZERO);
    let mut got = render_ready(&mut mixer, 49 * 256, 256);
    let edit_at = mixer.position();
    assert_eq!(edit_at, 49 * 256);
    handle.set_segments(&after).unwrap();
    got.extend(render_ready(&mut mixer, 48_000, 256));
    let switch = edit_at + sample_of(secs(0.1)); // default edit latency
    let end = RATE.samples_to_time(edit_at + 48_000).unwrap();
    let old = offline(&sources, &before, Time::ZERO, end);
    let new = offline(&sources, &after, Time::ZERO, end);
    let s = switch as usize * 2;
    let e = got.len().min(old.len());
    assert_eq!(&got[..s], &old[..s], "before the switch: the old mix");
    assert_eq!(&got[s..e], &new[s..e], "from the switch: the new mix");
    assert_eq!(handle.stats().underrun_frames, 0);
}

/// Track gain changes ramp in at the next callback without re-rendering the lane (heard the
/// limiter latency later: the lanes run that far ahead of the output); the master gain (after
/// the limiter) ramps too, at once.
#[test]
fn gain_changes_ramp_without_rerendering() {
    let sources = factory(&[sine(250.0, 0.2, 0.0, RATE, 1)]);
    let at = |db| segments(&[Seg::new(1, 1, 1, secs(0.0), secs(3.0)).gains(0.0, db)]);
    let (mut mixer, handle) = mixer(&sources);
    handle.set_segments(&at(0.0)).unwrap();
    handle.seek(Time::ZERO);
    let _ = render_ready(&mut mixer, 9_600, 256);
    let edit_at = mixer.position() as usize;
    assert_eq!(edit_at, 38 * 256);
    handle.set_segments(&at(-6.0)).unwrap();
    let tail = render_ready(&mut mixer, 9_600, 256);
    let reference = offline(&sources, &at(0.0), Time::ZERO, secs(1.0));
    let g = db_to_gain(-6.0);
    let ramp = sample_of(secs(0.01)) as usize;
    let latency = mixer.latency();
    assert!(latency > 0 && latency < ramp);
    for (i, f) in tail.as_chunks::<2>().0.iter().enumerate() {
        let full = reference[(edit_at + i) * 2];
        if i < latency {
            // Mixed before the change: the old gain, exactly.
            assert_eq!(f[0], full, "frame {i} before the change is heard");
        } else if i >= latency + ramp {
            assert!((f[0] - full * g).abs() <= 1e-6, "frame {i} after the ramp");
        } else {
            // Inside the ramp: between the old and the new gain.
            let (lo, hi) = (full * g, full);
            assert!(f[0] >= lo.min(hi) - 1e-6 && f[0] <= lo.max(hi) + 1e-6);
        }
    }
    // Master gain.
    handle.set_master_gain(-12.0);
    let pos = mixer.position() as usize;
    let more = render_ready(&mut mixer, 4_800, 256);
    let m = db_to_gain(-12.0);
    for (i, f) in more.as_chunks::<2>().0.iter().enumerate().skip(ramp) {
        let want = reference[(pos + i) * 2] * g * m;
        assert!((f[0] - want).abs() <= 1e-6, "master frame {i}");
    }
    let stats = handle.stats();
    assert_eq!(stats.underrun_frames, 0);
}

/// Changes made while nothing renders coalesce: the plan queue fills, later changes wait for
/// room, and the mixer ends up exactly on the last state.
#[test]
fn changes_coalesce_while_the_mixer_is_idle() {
    let sources = factory(&[
        sine(250.0, 0.2, 0.0, RATE, 1),
        sine(400.0, 0.2, 0.0, RATE, 2),
    ]);
    let (mut mixer, handle) = mixer(&sources);
    let layout_at = |i: u32| {
        segments(&[
            Seg::new(1, 1, 1 + u128::from(i % 2), secs(0.0), secs(2.0)).gains(-f64::from(i), 0.0)
        ])
    };
    for i in 0..30 {
        handle.set_segments(&layout_at(i)).unwrap();
    }
    handle.seek(secs(0.5));
    let stats = handle.stats();
    assert!(stats.plans_pending, "the queue filled up");
    assert_eq!(stats.plans_sent, 8);
    // The real-time side drains the queue; worker 0 delivers the coalesced rest.
    let mut spins = 0;
    while !handle.stats().settled() {
        let _ = mixer.ready(0);
        std::thread::sleep(StdDuration::from_millis(1));
        spins += 1;
        assert!(spins < 5_000, "never settled: {:?}", handle.stats());
    }
    assert_eq!(handle.stats().plans_sent, 9);
    assert_eq!(mixer.position(), sample_of(secs(0.5)));
    let got = render_ready(&mut mixer, 9_600, 480);
    assert_eq!(got, offline(&sources, &layout_at(29), secs(0.5), secs(0.7)));
}

/// Media that the factory cannot open play silence, are counted, and do not disturb the rest.
#[test]
fn failing_sources_are_silent_and_counted() {
    let sources = factory(&[sine(250.0, 0.2, 0.0, RATE, 1)]);
    let segs = segments(&[
        Seg::new(1, 1, 1, secs(0.0), secs(1.0)),
        Seg::new(2, 2, 99, secs(0.0), secs(1.0)),
    ]);
    let (mut mixer, handle) = mixer(&sources);
    handle.set_segments(&segs).unwrap();
    handle.seek(Time::ZERO);
    let got = render_ready(&mut mixer, 24_000, 512);
    let want = offline(
        &sources,
        &segs,
        Time::ZERO,
        RATE.samples_to_time(24_064).unwrap(),
    );
    assert_eq!(got, want);
    // Once per opened lane (the plan before the seek may have opened it too).
    assert!(handle.stats().source_errors >= 1);
}

#[test]
fn configs_are_validated() {
    let sources = factory(&[]);
    let mut c = MixerConfig::new(RATE);
    c.prefetch_ms = 499;
    assert!(Mixer::new(c, sources.clone()).is_err());
    let mut c = MixerConfig::new(RATE);
    c.workers = 0;
    assert!(Mixer::new(c, sources.clone()).is_err());
    let mut c = MixerConfig::new(RATE);
    c.prime_ms = 600;
    assert!(Mixer::new(c, sources).is_err());
}

/// Media pool for the property test.
fn pool() -> Vec<ToneSpec> {
    vec![
        sine(440.0, 0.1, 0.0, RATE, 1),
        sine(1234.5, 0.08, 1.0, SampleRate::HZ_44_100, 2),
        sine(97.0, 0.05, 2.0, SampleRate::HZ_96_000, 6),
        ToneSpec::new(
            Tone::Impulse {
                amplitude: 0.2,
                period: 333,
            },
            PcmFormat::new(RATE, 2),
        ),
        sine(5000.0, 0.1, 0.5, SampleRate::HZ_44_100, 1),
    ]
}

/// Clip layouts: up to 4 tracks of up to 3 non-overlapping clips, boundaries on 29.97 fps
/// frames (between samples), any source offset and gain.
fn layouts() -> impl Strategy<Value = Vec<Seg>> {
    let clip = (0u32..10, 1u32..20, 0u32..60, 0u128..5, -12i32..6);
    let track = (prop::collection::vec(clip, 1..4), -12i32..3);
    prop::collection::vec(track, 1..5).prop_map(|tracks| {
        let fps = FrameRate::FPS_29_97;
        let frame = |n: u32| fps.frame_to_time(i64::from(n)).unwrap();
        let mut segs = Vec::new();
        for (t, (clips, track_db)) in tracks.into_iter().enumerate() {
            let mut at = 0;
            for (k, (gap, len, src, media, clip_db)) in clips.into_iter().enumerate() {
                let start = at + gap;
                at = start + len;
                segs.push(
                    Seg::new(
                        t as u128 + 1,
                        (t * 10 + k) as u128 + 1,
                        media + 1,
                        frame(start),
                        frame(at),
                    )
                    .src(frame(src))
                    .gains(f64::from(clip_db), f64::from(track_db)),
                );
            }
        }
        segs
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// WBS property: offline == real-time-render-to-buffer within 1e-6, for any layout, buffer
    /// size and start position.
    #[test]
    fn offline_equals_realtime(
        segs in layouts(),
        buffer in 1usize..1500,
        start_frame in 0u32..40,
    ) {
        let sources = factory(&pool());
        let segs = segments(&segs);
        let start = FrameRate::FPS_29_97.frame_to_time(i64::from(start_frame)).unwrap();
        let frames = 36_000usize;
        let end = RATE.samples_to_time(sample_of(start) + frames as i64).unwrap();
        let reference = offline(&sources, &segs, start, end);
        prop_assume!(reference.iter().any(|&s| s != 0.0), "silent range");

        let (mut mixer, handle) = mixer(&sources);
        handle.set_segments(&segs).unwrap();
        handle.seek(start);
        let got = render_ready(&mut mixer, frames, buffer);
        let (diff, at) = max_diff(&got[..frames * 2], &reference);
        prop_assert!(diff <= 1e-6, "max diff {} at sample {}", diff, at);
        prop_assert_eq!(handle.stats().underrun_frames, 0);
    }
}
