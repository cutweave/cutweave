// SPDX-License-Identifier: GPL-3.0-or-later
//! Resampling quality (WP-1G.01): a 1 kHz sine at 44.1 kHz mixed at 48 kHz has THD+N below
//! −90 dB, and the resampler delay is trimmed so the sine stays on the timeline.
#![allow(
    clippy::many_single_char_names,
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod common;

use std::f64::consts::TAU;

use common::{RATE, Seg, factory, offline, secs, segments, sine};
use cutweave_model::{SampleRate, Time};

/// Least-squares fit of `a·sin(ωn) + b·cos(ωn) + c` to `x`; returns (THD+N in dB, delay of the
/// fitted sine in output samples relative to `sin(ωn)`).
fn thd_n(x: &[f64], first: usize, freq: f64) -> (f64, f64) {
    let w = TAU * freq / f64::from(RATE.hz());
    // Normal equations of the 3-parameter fit.
    let (mut ss, mut sc, mut cc, mut s1, mut c1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut xs, mut xc, mut x1) = (0.0, 0.0, 0.0);
    let n = x.len() as f64;
    for (i, &v) in x.iter().enumerate() {
        let t = w * (first + i) as f64;
        let (s, c) = t.sin_cos();
        ss += s * s;
        sc += s * c;
        cc += c * c;
        s1 += s;
        c1 += c;
        xs += v * s;
        xc += v * c;
        x1 += v;
    }
    let m = [[ss, sc, s1], [sc, cc, c1], [s1, c1, n]];
    let r = [xs, xc, x1];
    let det = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(m);
    let solve = |col: usize| {
        let mut mm = m;
        for row in 0..3 {
            mm[row][col] = r[row];
        }
        det(mm) / d
    };
    let (a, b, c) = (solve(0), solve(1), solve(2));
    let (mut signal, mut residual) = (0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let t = w * (first + i) as f64;
        let fit = a * t.sin() + b * t.cos();
        signal += fit * fit;
        residual += (v - fit - c).powi(2);
    }
    let phase = b.atan2(a); // a·sin + b·cos = A·sin(ωn + phase)
    (10.0 * (residual / signal).log10(), -phase / w)
}

#[test]
fn resampling_44k1_to_48k_has_thd_n_below_minus_90_db() {
    let sources = factory(&[sine(1000.0, 0.5, 0.0, SampleRate::HZ_44_100, 1)]);
    let segs = segments(&[Seg::new(1, 1, 1, Time::ZERO, secs(3.0))]);
    let mix = offline(&sources, &segs, Time::ZERO, secs(3.0));
    // 2 s (2000 whole periods at 48 kHz) away from the clip edges.
    let first = 24_000;
    let left: Vec<f64> = mix
        .iter()
        .step_by(2)
        .skip(first)
        .take(96_000)
        .map(|&s| f64::from(s))
        .collect();
    let (db, delay) = thd_n(&left, first, 1000.0);
    eprintln!("44.1k → 48k, 1 kHz: THD+N {db:.1} dB, timing error {delay:+.4} samples");
    assert!(db < -90.0, "THD+N {db:.1} dB");
    assert!(
        delay.abs() < 0.5,
        "resampler delay not trimmed: {delay} samples"
    );
}

#[test]
fn resampling_96k_to_48k_keeps_quality_and_timing() {
    let sources = factory(&[sine(1000.0, 0.5, 0.0, SampleRate::HZ_96_000, 2)]);
    let segs = segments(&[Seg::new(1, 1, 1, Time::ZERO, secs(3.0))]);
    let mix = offline(&sources, &segs, Time::ZERO, secs(3.0));
    let first = 24_000;
    let right: Vec<f64> = mix
        .iter()
        .skip(1)
        .step_by(2)
        .skip(first)
        .take(96_000)
        .map(|&s| f64::from(s))
        .collect();
    let (db, delay) = thd_n(&right, first, 1000.0);
    eprintln!("96k → 48k, 1 kHz: THD+N {db:.1} dB, timing error {delay:+.4} samples");
    assert!(db < -90.0, "THD+N {db:.1} dB");
    assert!(
        delay.abs() < 0.5,
        "resampler delay not trimmed: {delay} samples"
    );
}
