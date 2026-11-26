// SPDX-License-Identifier: GPL-3.0-or-later
//! Bus graph, master limiter and meter acceptance tests (WP-2E.02): the true-peak ceiling on a
//! +6 dBFS sine, exclusive solo-in-place with mute, bus changes that ramp in without
//! re-rendering lanes, the meters, and the offline level analysis behind `audio.get_levels`
//! (−20 dBFS tone, −23 LUFS pink noise, EBU Tech 3341 case 1).
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

use common::{RATE, Seg, factory, max_diff, render_ready, sample_of, secs, segments, sine};
use cutweave_audio::{
    BusGraph, BusLevel, BusParams, LimiterConfig, Mixer, MixerConfig, PcmFormat, SourceFactory,
    Tone, ToneSpec, analyze_levels, mixdown_with, to_db,
};
use cutweave_model::{Time, TrackId};
use cutweave_render::AudioSegment;

fn track(n: u128) -> TrackId {
    TrackId::from_u128(n)
}

/// The whole offline mix of `[start, end)` with the bus settings `buses` and `limiter`.
fn mix(
    sources: &Arc<dyn SourceFactory>,
    segs: &[AudioSegment],
    buses: &BusGraph,
    end: Time,
    limiter: LimiterConfig,
) -> Vec<f32> {
    let mut out = Vec::new();
    for chunk in mixdown_with(sources, segs, buses, Time::ZERO..end, RATE, limiter).unwrap() {
        out.extend_from_slice(&chunk.samples);
    }
    out
}

/// True peak (dBTP, both channels) with ebur128, the measurement the WBS names.
fn true_peak_db(audio: &[f32]) -> f64 {
    let mut m = ebur128::EbuR128::new(2, RATE.hz(), ebur128::Mode::TRUE_PEAK).unwrap();
    m.add_frames_f32(audio).unwrap();
    to_db(m.true_peak(0).unwrap().max(m.true_peak(1).unwrap()))
}

fn stereo_dc(level: f32) -> ToneSpec {
    ToneSpec::new(Tone::Dc { level }, PcmFormat::new(RATE, 2))
}

// ----- the limiter -------------------------------------------------------------------------------

/// WBS 2E.02: a +6 dBFS sine through the whole graph comes out with a true peak of at most the
/// ceiling + 0.1 dB, over 10 s, for low, mid and high frequencies and two ceilings.
#[test]
fn a_plus_6_dbfs_sine_stays_under_the_true_peak_ceiling_for_10_s() {
    for (freq, ceiling) in [(997.0, -1.0), (100.0, -1.0), (7_351.0, -1.0), (997.0, -3.0)] {
        // A full-scale sine with +6.02 dB of clip gain: peaks at +6 dBFS.
        let sources = factory(&[sine(freq, 1.0, 0.0, RATE, 2)]);
        let segs = segments(&[Seg::new(1, 1, 1, secs(0.0), secs(10.0)).gains(6.0206, 0.0)]);
        let limiter = LimiterConfig {
            ceiling_dbtp: ceiling,
            ..LimiterConfig::default()
        };
        let out = mix(
            &sources,
            &segs,
            &BusGraph::from_segments(&segs),
            secs(10.0),
            limiter,
        );
        assert_eq!(out.len(), 2 * 480_000);
        let tp = true_peak_db(&out);
        eprintln!("{freq} Hz into a {ceiling} dBTP ceiling: {tp:.3} dBTP");
        assert!(tp <= ceiling + 0.1, "{freq} Hz: {tp:.3} dBTP");
        assert!(tp > ceiling - 1.0, "{freq} Hz: crushed to {tp:.3} dBTP");
        // Without the limiter the same mix keeps its +6 dB.
        let raw = mix(
            &sources,
            &segs,
            &BusGraph::from_segments(&segs),
            secs(1.0),
            LimiterConfig::disabled(),
        );
        assert!(true_peak_db(&raw) > 5.9);
    }
}

/// The limiter is the same in playback and export: a hot mix rendered in real time with
/// odd buffer sizes equals the offline mixdown sample for sample.
#[test]
fn the_limited_real_time_mix_equals_the_mixdown() {
    let sources = factory(&[
        sine(997.0, 0.9, 0.0, RATE, 2),
        sine(220.0, 0.8, 0.3, cutweave_model::SampleRate::HZ_44_100, 1),
    ]);
    let segs = segments(&[
        Seg::new(1, 1, 1, secs(0.0), secs(2.0)).gains(3.0, 0.0),
        Seg::new(2, 2, 2, secs(0.3), secs(1.7)).gains(0.0, 2.0),
    ]);
    let buses = BusGraph::from_segments(&segs);
    let want = mix(&sources, &segs, &buses, secs(1.5), LimiterConfig::default());
    for buffer in [256usize, 333, 1024] {
        let (mut mixer, handle) = Mixer::new(MixerConfig::new(RATE), sources.clone()).unwrap();
        handle.set_mix(&segs, &buses).unwrap();
        handle.seek(Time::ZERO);
        let got = render_ready(&mut mixer, want.len() / 2, buffer);
        let (diff, at) = max_diff(&got[..want.len()], &want);
        assert_eq!(diff, 0.0, "buffer {buffer}: diff {diff} at {at}");
        assert!(mixer.take_limiter_gain() < 0.9, "the limiter worked");
        assert_eq!(handle.stats().underrun_frames, 0);
    }
}

// ----- mute and solo -----------------------------------------------------------------------------

/// Four tracks of stereo DC at 1/64, 2/64, 4/64 and 8/64: the mix level × 64 spells which
/// tracks are heard (bit `n − 1` for track `n`).
fn heard(buses: &BusGraph) -> u32 {
    let sources = factory(&[
        stereo_dc(1.0 / 64.0),
        stereo_dc(2.0 / 64.0),
        stereo_dc(4.0 / 64.0),
        stereo_dc(8.0 / 64.0),
    ]);
    let segs = segments(
        &(1..=4)
            .map(|n| Seg::new(n, n, n, secs(0.0), secs(0.1)))
            .collect::<Vec<_>>(),
    );
    let out = mix(&sources, &segs, buses, secs(0.1), LimiterConfig::default());
    let v = out[out.len() - 2];
    assert_eq!(out[out.len() - 1], v);
    let bits = (v * 64.0).round();
    assert!((v * 64.0 - bits).abs() < 1e-4, "{v}");
    bits as u32
}

fn buses(spec: &[(u128, bool, bool)]) -> BusGraph {
    let mut g = BusGraph::new();
    for n in 1..=4 {
        g.set(BusParams::new(track(n)));
    }
    for &(n, muted, solo) in spec {
        g.set(BusParams {
            muted,
            solo,
            ..BusParams::new(track(n))
        });
    }
    g
}

#[test]
fn solo_and_mute_decide_what_is_heard() {
    // Nothing soloed: every unmuted track.
    assert_eq!(heard(&buses(&[])), 0b1111);
    assert_eq!(heard(&buses(&[(2, true, false)])), 0b1101);
    // One solo: only it.
    assert_eq!(heard(&buses(&[(3, false, true)])), 0b0100);
    // Several solos: all of them, nothing else.
    assert_eq!(heard(&buses(&[(1, false, true), (4, false, true)])), 0b1001);
    // A muted track stays muted when another is soloed.
    assert_eq!(heard(&buses(&[(1, true, false), (2, false, true)])), 0b0010);
    // Mute wins over solo, and the muted solo still silences the rest.
    assert_eq!(heard(&buses(&[(1, true, true)])), 0);
    assert_eq!(heard(&buses(&[(1, true, true), (3, false, true)])), 0b0100);
    // Soloing a muted track and unmuting it later.
    assert_eq!(heard(&buses(&[(2, false, true)])), 0b0010);
}

/// A bus's pan and gain: hard left silences the right channel; −6.02 dB halves the level.
#[test]
fn bus_gain_and_pan() {
    let sources = factory(&[stereo_dc(0.25)]);
    let segs = segments(&[Seg::new(1, 1, 1, secs(0.0), secs(0.1))]);
    let g = BusGraph::new().with(BusParams {
        pan: -1.0,
        gain_db: -6.020_6,
        ..BusParams::new(track(1))
    });
    let out = mix(&sources, &segs, &g, secs(0.1), LimiterConfig::default());
    let last = &out[out.len() - 2..];
    assert!((last[0] - 0.125).abs() < 1e-5, "{last:?}");
    assert!(last[1].abs() < 1e-7, "{last:?}");
}

/// Muting and soloing during playback ramp in at the next callback (heard the limiter latency
/// later) instead of waiting for a lane hand-off `edit_latency` (100 ms) ahead: the lanes keep
/// their audio.
#[test]
fn mute_and_solo_during_playback_ramp_in_without_new_lanes() {
    let sources = factory(&[stereo_dc(0.125), stereo_dc(0.25)]);
    let segs = segments(&[
        Seg::new(1, 1, 1, secs(0.0), secs(3.0)),
        Seg::new(2, 2, 2, secs(0.0), secs(3.0)),
    ]);
    let both = BusGraph::new()
        .with(BusParams::new(track(1)))
        .with(BusParams::new(track(2)));
    let (mut mixer, handle) = Mixer::new(MixerConfig::new(RATE), sources).unwrap();
    handle.set_mix(&segs, &both).unwrap();
    handle.seek(Time::ZERO);
    let before = render_ready(&mut mixer, 4800, 256);
    assert_eq!(before[before.len() - 2], 0.375);
    let latency = mixer.latency();
    let ramp = sample_of(secs(0.01)) as usize;
    // Solo track 1: track 2 fades out over 10 ms from the next callback.
    handle
        .set_buses(&both.clone().with(BusParams {
            solo: true,
            ..BusParams::new(track(1))
        }))
        .unwrap();
    let after = render_ready(&mut mixer, 2400, 256);
    let left: Vec<f32> = after.iter().step_by(2).copied().collect();
    assert!(left[..latency].iter().all(|&v| v == 0.375));
    assert!(left[latency + 1] < 0.375 && left[latency + 1] > 0.125);
    assert!(
        left[latency + ramp..].iter().all(|&v| v == 0.125),
        "solo in effect"
    );
    // Mute track 1 too: silence, again within the ramp.
    handle
        .set_buses(&both.clone().with(BusParams {
            solo: true,
            muted: true,
            ..BusParams::new(track(1))
        }))
        .unwrap();
    let muted = render_ready(&mut mixer, 2400, 256);
    assert!(muted[(latency + ramp) * 2..].iter().all(|&v| v == 0.0));
    // Back to both: no hand-off, so no new lane and no underrun.
    handle.set_buses(&both).unwrap();
    let back = render_ready(&mut mixer, 2400, 256);
    assert_eq!(back[back.len() - 2], 0.375);
    assert_eq!(handle.stats().underrun_frames, 0);
    assert_eq!(handle.buses(), both);
}

/// The bus meters read post-fader peak and RMS; a muted bus reads silence.
#[test]
fn bus_meters_read_post_fader_levels() {
    let sources = factory(&[stereo_dc(0.25), sine(1000.0, 0.5, 0.0, RATE, 2)]);
    let segs = segments(&[
        Seg::new(1, 1, 1, secs(0.0), secs(2.0)),
        Seg::new(2, 2, 2, secs(0.0), secs(2.0)),
        Seg::new(3, 3, 1, secs(0.0), secs(2.0)),
    ]);
    let g = BusGraph::new()
        .with(BusParams {
            gain_db: -6.020_6,
            ..BusParams::new(track(1))
        })
        .with(BusParams::new(track(2)))
        .with(BusParams {
            muted: true,
            ..BusParams::new(track(3))
        });
    let (mut mixer, handle) = Mixer::new(MixerConfig::new(RATE), sources).unwrap();
    handle.set_mix(&segs, &g).unwrap();
    handle.seek(Time::ZERO);
    let _ = render_ready(&mut mixer, 4800, 480);
    let mut levels = [BusLevel::SILENT; 4];
    let _ = mixer.take_bus_levels(&mut levels);
    let _ = render_ready(&mut mixer, 9600, 480);
    let n = mixer.take_bus_levels(&mut levels);
    assert_eq!(n, 3);
    let of = |t: u128| levels.iter().find(|l| l.track == track(t)).unwrap();
    assert!((of(1).peak[0] - 0.125).abs() < 1e-5 && (of(1).rms[1] - 0.125).abs() < 1e-5);
    assert!((of(2).peak[0] - 0.5).abs() < 1e-3);
    assert!((of(2).rms[0] - 0.5 / std::f32::consts::SQRT_2).abs() < 1e-3);
    assert_eq!(of(3).peak, [0.0, 0.0]);
    assert_eq!(of(3).rms, [0.0, 0.0]);
    // Taking restarts the window.
    let _ = mixer.take_bus_levels(&mut levels);
    assert_eq!(mixer.take_bus_levels(&mut levels), 3);
    assert_eq!(levels[0].peak, [0.0, 0.0]);
}

// ----- audio.get_levels: the offline analysis ---------------------------------------------------

/// WBS 2E.02: a −20 dBFS tone reads −20 ± 0.2 dB RMS. `audio.get_levels` reports RMS on the
/// AES17 scale (a sine reads its peak level: raw RMS + 3.01 dB), so the raw RMS here is −23.01.
#[test]
fn get_levels_of_a_minus_20_dbfs_tone() {
    let sources = factory(&[sine(997.0, 0.1, 0.0, RATE, 2)]);
    let segs = segments(&[Seg::new(1, 1, 1, secs(0.0), secs(5.0))]);
    let report = analyze_levels(
        &sources,
        &segs,
        &BusGraph::from_segments(&segs),
        Time::ZERO..secs(5.0),
        RATE,
        LimiterConfig::default(),
    )
    .unwrap();
    let m = report.master;
    let aes17 = |rms: f32| to_db(f64::from(rms)) + 3.0103;
    for c in 0..2 {
        assert!((to_db(f64::from(m.peak[c])) + 20.0).abs() < 0.05);
        assert!((aes17(m.rms[c]) + 20.0).abs() <= 0.2, "{}", aes17(m.rms[c]));
        assert!((to_db(f64::from(m.true_peak[c])) + 20.0).abs() < 0.1);
    }
    // A 997 Hz stereo sine at −20 dBFS: −20 LUFS (K-weighting ≈ +0.7 dB at 1 kHz cancels the
    // −0.691 offset; two channels add 3 dB to the −3 dB of a sine's mean square).
    assert!((m.integrated + 20.0).abs() < 0.2, "{}", m.integrated);
    assert_eq!(report.tracks.len(), 1);
    assert!((aes17(report.tracks[0].levels.rms[0]) + 20.0).abs() <= 0.2);
    assert_eq!(report.limiter_gain, 1.0);
    assert_eq!(report.samples, 0..240_000);
    assert_eq!(report.source_errors, 0);
}

/// WBS 2E.02: a −23 LUFS pink-noise reference reads −23 ± 0.3 LUFS. The seeded pink noise is
/// scaled to −23 LUFS by measuring the source itself with ebur128, then analysed through the
/// whole graph (a mono source panned centre, a bus at unity, the limiter on).
#[test]
fn get_levels_of_minus_23_lufs_pink_noise() {
    let noise = ToneSpec::new(
        Tone::PinkNoise {
            amplitude: 1.0,
            seed: 0x2E02,
        },
        PcmFormat::new(RATE, 2),
    );
    // Its loudness at unity, measured directly.
    let mut m = ebur128::EbuR128::new(2, RATE.hz(), ebur128::Mode::I).unwrap();
    let raw: Vec<f32> = (0..48_000u64 * 20)
        .flat_map(|n| {
            let v = noise.sample(n);
            [v, v]
        })
        .collect();
    m.add_frames_f32(&raw).unwrap();
    let unity = m.loudness_global().unwrap();
    let gain = -23.0 - unity;
    let sources = factory(&[noise]);
    let segs = segments(&[Seg::new(1, 1, 1, secs(0.0), secs(20.0)).gains(gain, 0.0)]);
    let report = analyze_levels(
        &sources,
        &segs,
        &BusGraph::from_segments(&segs),
        Time::ZERO..secs(20.0),
        RATE,
        LimiterConfig::default(),
    )
    .unwrap();
    let i = report.master.integrated;
    eprintln!("pink noise at unity: {unity:.2} LUFS; scaled by {gain:.2} dB: {i:.3} LUFS");
    assert!((i + 23.0).abs() <= 0.3, "{i}");
    assert!((report.tracks[0].levels.integrated + 23.0).abs() <= 0.3);
    let short = report.master.max_short_term.unwrap();
    assert!(short > -24.0 && short < -21.0, "{short}");
    assert!(report.master.range.unwrap() >= 0.0);
}

/// EBU Tech 3341 test 1 through `analyze_levels`: a stereo 1 kHz sine at −23 dBFS reads −23.0
/// LUFS momentary, short-term and integrated (± 0.1).
#[test]
fn get_levels_matches_tech_3341_case_1() {
    let amplitude = 10f32.powf(-23.0 / 20.0);
    let sources = factory(&[sine(1000.0, amplitude, 0.0, RATE, 2)]);
    let segs = segments(&[Seg::new(1, 1, 1, secs(0.0), secs(20.0))]);
    let r = analyze_levels(
        &sources,
        &segs,
        &BusGraph::from_segments(&segs),
        Time::ZERO..secs(20.0),
        RATE,
        LimiterConfig::default(),
    )
    .unwrap()
    .master;
    assert!((r.integrated + 23.0).abs() <= 0.1, "{}", r.integrated);
    assert!((r.max_momentary.unwrap() + 23.0).abs() <= 0.1);
    assert!((r.max_short_term.unwrap() + 23.0).abs() <= 0.1);
}

/// The analysis follows the bus graph and the limiter: a muted track reads silence, a hot
/// master is limited (or not, with the limiter off), and a range in the middle is measured
/// exactly.
#[test]
fn get_levels_follows_mute_solo_and_the_limiter() {
    let sources = factory(&[sine(500.0, 1.0, 0.0, RATE, 2), stereo_dc(0.1)]);
    let segs = segments(&[
        Seg::new(1, 1, 1, secs(0.0), secs(2.0)).gains(6.0, 0.0),
        Seg::new(2, 2, 2, secs(0.0), secs(2.0)),
    ]);
    let g = BusGraph::from_segments(&segs).with(BusParams {
        muted: true,
        ..BusParams::new(track(2))
    });
    let limited = analyze_levels(
        &sources,
        &segs,
        &g,
        secs(0.5)..secs(1.5),
        RATE,
        LimiterConfig::default(),
    )
    .unwrap();
    assert_eq!(limited.samples, 24_000..72_000);
    assert!(limited.limiter_gain < 0.5);
    let tp = to_db(f64::from(limited.master.true_peak[0]));
    assert!(tp <= -0.9, "{tp}");
    let muted = limited.tracks.iter().find(|t| t.track == track(2)).unwrap();
    assert_eq!(muted.levels.peak, [0.0, 0.0]);
    assert_eq!(muted.levels.integrated, f64::NEG_INFINITY);
    let open = analyze_levels(
        &sources,
        &segs,
        &g,
        secs(0.5)..secs(1.5),
        RATE,
        LimiterConfig::disabled(),
    )
    .unwrap();
    assert_eq!(open.limiter_gain, 1.0);
    assert!(to_db(f64::from(open.master.peak[0])) > 5.9);
}
