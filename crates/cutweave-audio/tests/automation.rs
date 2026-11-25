// SPDX-License-Identifier: GPL-3.0-or-later
//! Clip automation acceptance tests (WP-2E.01): fades, keyframed gain and pan, click-freedom, and
//! preview == export (the real-time mixer against the offline mixdown, the null test).
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

use common::{RATE, Seg, factory, offline, render_ready, sample_of, secs, segments, sine};
use cutweave_audio::{Mixer, MixerConfig, PcmFormat, SourceFactory, Tone, ToneSpec};
use cutweave_model::{
    Animatable, Duration, Fade, FadeCurve, Interp, Keyframe, Keyframes, SampleRate, Time,
};

fn dur(s: f64) -> Duration {
    Duration::from_flicks(secs(s).flicks()).unwrap()
}

fn db_of(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// Peak of `|x|` in the `frames` frames of the left channel around `centre`.
fn peak_near(mix: &[f32], centre: usize, frames: usize) -> f64 {
    (centre - frames..=centre + frames)
        .map(|n| f64::from(mix[n * 2].abs()))
        .fold(0.0, f64::max)
}

#[test]
fn a_one_second_linear_fade_in_is_half_amplitude_at_half_a_second() {
    // Below the limiter knee: amplitude 0.8 at full gain.
    let sources = factory(&[sine(1000.0, 0.8, 0.0, RATE, 2)]);
    let mut seg = Seg::new(1, 1, 1, Time::ZERO, secs(3.0));
    seg.fade_in = Fade {
        duration: dur(1.0),
        curve: FadeCurve::Linear,
    };
    let mix = offline(&sources, &segments(&[seg]), Time::ZERO, secs(3.0));
    let at = sample_of(secs(0.5)) as usize;
    let ratio = peak_near(&mix, at, 24) / 0.8;
    assert!((ratio - 0.5).abs() < 0.01, "0.5 s: {ratio}");
    // Silence at the very start, full level after the fade.
    assert!(peak_near(&mix, 24, 24) / 0.8 < 0.001);
    let after = sample_of(secs(1.5)) as usize;
    assert!((peak_near(&mix, after, 24) / 0.8 - 1.0).abs() < 0.01);
}

#[test]
fn fades_in_and_out_reach_silence_at_the_clip_edges() {
    let sources = factory(&[ToneSpec::new(
        Tone::Dc { level: 0.5 },
        PcmFormat::new(RATE, 2),
    )]);
    for curve in [
        FadeCurve::Linear,
        FadeCurve::Log,
        FadeCurve::EqualPower,
        FadeCurve::SCurve,
    ] {
        let mut seg = Seg::new(1, 1, 1, secs(1.0), secs(2.0));
        let fade = Fade {
            duration: dur(0.25),
            curve,
        };
        seg.fade_in = fade;
        seg.fade_out = fade;
        let mix = offline(&sources, &segments(&[seg]), Time::ZERO, secs(3.0));
        let left: Vec<f32> = mix.iter().step_by(2).copied().collect();
        let (a, b) = (sample_of(secs(1.0)) as usize, sample_of(secs(2.0)) as usize);
        assert_eq!(left[a - 1], 0.0, "{curve:?}");
        assert_eq!(
            left[a], 0.0,
            "{curve:?}: the first sample of a fade in is silent"
        );
        assert!(left[b - 1].abs() < 0.001, "{curve:?}: ends in silence");
        assert_eq!(left[b], 0.0);
        assert!(
            (left[usize::midpoint(a, b)] - 0.5).abs() < 1e-6,
            "{curve:?}: full level between"
        );
        // No step anywhere above the ramp limit.
        let worst = left
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(worst < 0.02, "{curve:?}: step {worst}");
    }
}

#[test]
fn fades_longer_than_the_clip_scale_down_to_fit() {
    let sources = factory(&[ToneSpec::new(
        Tone::Dc { level: 0.5 },
        PcmFormat::new(RATE, 2),
    )]);
    let mut seg = Seg::new(1, 1, 1, Time::ZERO, secs(1.0));
    let fade = Fade {
        duration: dur(2.0),
        curve: FadeCurve::Linear,
    };
    seg.fade_in = fade;
    seg.fade_out = fade;
    let mix = offline(&sources, &segments(&[seg]), Time::ZERO, secs(1.0));
    let left: Vec<f32> = mix.iter().step_by(2).copied().collect();
    // Half the clip each: the peak of the triangle is in the middle.
    let mid = left.len() / 2;
    assert!((left[mid] - 0.5).abs() < 0.01, "{}", left[mid]);
    assert!(left[mid / 2] < left[mid] && left[mid + mid / 2] < left[mid]);
}

#[test]
fn centred_mono_is_minus_3_db_per_channel_and_hard_pans_are_full_scale_on_one_side() {
    let sources = factory(&[sine(1000.0, 0.5, 0.0, RATE, 1)]);
    let level = |pan: f64| {
        let mut seg = Seg::new(1, 1, 1, Time::ZERO, secs(1.0));
        seg.pan = Animatable::Static(pan);
        let mix = offline(&sources, &segments(&[seg]), Time::ZERO, secs(1.0));
        let peak = |c: usize| {
            mix.iter()
                .skip(c)
                .step_by(2)
                .map(|s| f64::from(s.abs()))
                .fold(0.0, f64::max)
        };
        (peak(0) / 0.5, peak(1) / 0.5)
    };
    let (l, r) = level(0.0);
    assert!((db_of(l) + 3.0).abs() < 0.1, "left {} dB", db_of(l));
    assert!((db_of(r) + 3.0).abs() < 0.1, "right {} dB", db_of(r));
    let (l, r) = level(-1.0);
    assert!((l - 1.0).abs() < 1e-4 && r < 1e-6, "{l} {r}");
    let (l, r) = level(1.0);
    assert!(l < 1e-6 && (r - 1.0).abs() < 1e-4, "{l} {r}");
}

#[test]
fn a_stereo_clip_is_unchanged_at_centre_and_balanced_to_one_side() {
    let sources = factory(&[sine(1000.0, 0.5, 0.0, RATE, 2)]);
    let render = |pan: f64| {
        let mut seg = Seg::new(1, 1, 1, Time::ZERO, secs(0.5));
        seg.pan = Animatable::Static(pan);
        offline(&sources, &segments(&[seg]), Time::ZERO, secs(0.5))
    };
    let plain = render(0.0);
    let reference = offline(
        &sources,
        &segments(&[Seg::new(1, 1, 1, Time::ZERO, secs(0.5))]),
        Time::ZERO,
        secs(0.5),
    );
    assert_eq!(plain, reference);
    let right = render(1.0);
    assert!(
        right.iter().step_by(2).all(|&s| s.abs() < 1e-6),
        "left is silent"
    );
}

/// A hold keyframe jumps the gain between two adjacent keys; the mix ramps over at most 64
/// frames instead of stepping.
#[test]
fn a_keyframed_volume_jump_has_no_click() {
    let sources = factory(&[ToneSpec::new(
        Tone::Dc { level: 0.5 },
        PcmFormat::new(RATE, 2),
    )]);
    let t = |s: f64| secs(s);
    let keys = Keyframes::new(vec![
        Keyframe::new(t(1.0), 0.0, Interp::Hold),
        Keyframe::new(
            Time::from_flicks(t(1.0).flicks() + 14_700).unwrap(),
            -144.0,
            Interp::Hold,
        ),
        Keyframe::new(t(2.0), 0.0, Interp::Hold),
    ])
    .unwrap();
    let mut seg = Seg::new(1, 1, 1, Time::ZERO, secs(3.0));
    seg.gain = Some(Animatable::Keyframes(keys));
    let mix = offline(&sources, &segments(&[seg]), Time::ZERO, secs(3.0));
    let left: Vec<f32> = mix.iter().step_by(2).copied().collect();
    let at = |s: f64| sample_of(secs(s)) as usize;
    assert_eq!(left[at(0.5)], 0.5);
    assert_eq!(left[at(1.5)], 0.0, "muted between the keys");
    assert_eq!(left[at(2.5)], 0.5);
    let worst = left
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(worst < 0.02 * 0.5 * 2.0, "largest step {worst}");
    assert!(worst > 0.0);
}

/// Linear keyframes move the gain smoothly between the keys.
#[test]
fn keyframed_volume_follows_the_curve_between_keys() {
    let sources = factory(&[ToneSpec::new(
        Tone::Dc { level: 0.5 },
        PcmFormat::new(RATE, 2),
    )]);
    let keys = Keyframes::new(vec![
        Keyframe::new(secs(0.0), -20.0, Interp::Linear),
        Keyframe::new(secs(2.0), 0.0, Interp::Linear),
    ])
    .unwrap();
    let mut seg = Seg::new(1, 1, 1, secs(1.0), secs(4.0));
    seg.gain = Some(Animatable::Keyframes(keys));
    let mix = offline(&sources, &segments(&[seg]), Time::ZERO, secs(4.0));
    let left = |s: f64| f64::from(mix[sample_of(secs(s)) as usize * 2]);
    // Item time 0 is timeline 1.0 s; the middle key value is -10 dB at timeline 2.0 s.
    assert!((left(1.0) - 0.5 * 0.1).abs() < 1e-4);
    assert!((left(2.0) - 0.5 * 10f64.powf(-0.5)).abs() < 2e-3);
    assert!((left(3.5) - 0.5).abs() < 1e-6, "holds the last key");
}

/// Export equals preview: the real-time mixer (block sizes that do not align with the control
/// grid, a mid-clip seek) against the offline mixdown, over clips with every kind of automation.
#[test]
fn export_mix_equals_preview_mix() {
    let sources: Arc<dyn SourceFactory> = factory(&[
        sine(220.0, 0.2, 0.0, RATE, 1),
        sine(330.0, 0.2, 0.3, SampleRate::HZ_44_100, 2),
        sine(440.0, 0.2, 0.6, RATE, 2),
    ]);
    let gain = Keyframes::new(vec![
        Keyframe::new(secs(0.0), -12.0, Interp::Linear),
        Keyframe::new(secs(1.0), 0.0, Interp::Hold),
        Keyframe::new(secs(1.5), -30.0, Interp::Linear),
        Keyframe::new(secs(2.5), 3.0, Interp::Linear),
    ])
    .unwrap();
    let pan = Keyframes::new(vec![
        Keyframe::new(secs(0.0), -1.0, Interp::Linear),
        Keyframe::new(secs(2.0), 1.0, Interp::Linear),
    ])
    .unwrap();
    let mut a = Seg::new(1, 1, 1, secs(0.1), secs(3.0));
    a.gain = Some(Animatable::Keyframes(gain));
    a.pan = Animatable::Keyframes(pan);
    a.fade_in = Fade {
        duration: dur(0.4),
        curve: FadeCurve::EqualPower,
    };
    a.fade_out = Fade {
        duration: dur(0.7),
        curve: FadeCurve::Log,
    };
    let mut b = Seg::new(2, 2, 2, secs(0.3), secs(2.9)).gains(-4.0, -1.0);
    b.fade_in = Fade {
        duration: dur(0.5),
        curve: FadeCurve::SCurve,
    };
    b.pan = Animatable::Static(0.4);
    let mut c = Seg::new(3, 3, 3, secs(0.0), secs(2.5)).src(secs(0.2));
    c.fade_out = Fade {
        duration: dur(1.0),
        curve: FadeCurve::Linear,
    };
    let segs = segments(&[a, b, c]);

    let (mut mixer, handle) = Mixer::new(MixerConfig::new(RATE), sources.clone()).unwrap();
    handle.set_segments(&segs).unwrap();
    // Start mid-clip so the voices open inside their fades.
    handle.seek(secs(0.25));
    let start = sample_of(secs(0.25));
    let frames = 2 * 48_000 + 480;
    let preview = render_ready(&mut mixer, frames, 480);
    let end = RATE
        .samples_to_time(start + preview.len() as i64 / 2)
        .unwrap();
    let export = offline(&sources, &segs, secs(0.25), end);
    assert_eq!(preview.len(), export.len());
    let mut err = 0.0f64;
    let mut energy = 0.0f64;
    for (p, e) in preview.iter().zip(&export) {
        err += f64::from(p - e).powi(2);
        energy += f64::from(*e).powi(2);
    }
    assert!(energy > 1.0, "the mix is not silent");
    // Difference level against the full-scale reference, in dBFS (RMS of the difference).
    let diff_dbfs = 10.0 * (err / export.len() as f64).max(1e-30).log10();
    assert!(diff_dbfs < -90.0, "null test: {diff_dbfs} dBFS");
    assert_eq!(handle.stats().underrun_frames, 0);
}
