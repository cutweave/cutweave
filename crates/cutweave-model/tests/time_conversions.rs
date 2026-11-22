// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1A.01 acceptance: exhaustive frame/sample conversions at every ADR-0004 rate, exact
//! integer ratios, and overflow behaviour at the edges of `i64`.

use cutweave_model::{
    Duration, FLICKS_PER_SECOND, FrameRate, MAX_SAFE_FLICKS, Rounding, SampleRate, Time, TimeError,
};

/// The ADR-0004 divisibility table: (rate, flicks per frame).
const FRAME_TABLE: [(FrameRate, i64); 9] = [
    (FrameRate::FPS_23_976, 29_429_400),
    (FrameRate::FPS_24, 29_400_000),
    (FrameRate::FPS_25, 28_224_000),
    (FrameRate::FPS_29_97, 23_543_520),
    (FrameRate::FPS_30, 23_520_000),
    (FrameRate::FPS_50, 14_112_000),
    (FrameRate::FPS_59_94, 11_771_760),
    (FrameRate::FPS_60, 11_760_000),
    (FrameRate::FPS_120, 5_880_000),
];

const SAMPLE_TABLE: [(SampleRate, i64); 3] = [
    (SampleRate::HZ_44_100, 16_000),
    (SampleRate::HZ_48_000, 14_700),
    (SampleRate::HZ_96_000, 7_350),
];

const FRAMES: i64 = 1_000_000;

#[test]
fn every_common_rate_is_an_exact_integer_ratio() {
    for (rate, per_frame) in FRAME_TABLE {
        assert!(rate.is_exact(), "{rate}");
        assert_eq!(rate.flicks_per_frame(), Some(per_frame), "{rate}");
        // Integer identity, no float: flicks/s * den == per_frame * num.
        assert_eq!(
            i128::from(FLICKS_PER_SECOND) * i128::from(rate.den()),
            i128::from(per_frame) * i128::from(rate.num()),
            "{rate}"
        );
    }
    for (rate, per_sample) in SAMPLE_TABLE {
        assert!(rate.is_exact(), "{rate}");
        assert_eq!(rate.flicks_per_sample(), Some(per_sample), "{rate}");
        assert_eq!(
            FLICKS_PER_SECOND,
            per_sample * i64::from(rate.hz()),
            "{rate}"
        );
    }
    assert_eq!(FrameRate::COMMON, FRAME_TABLE.map(|(r, _)| r));
    assert_eq!(SampleRate::COMMON, SAMPLE_TABLE.map(|(r, _)| r));
}

#[test]
fn frames_round_trip_exactly_for_the_first_million_frames() {
    for (rate, per_frame) in FRAME_TABLE {
        let half = per_frame / 2;
        for n in 0..FRAMES {
            let t = rate.frame_to_time(n).unwrap();
            assert_eq!(t.flicks(), n * per_frame, "{rate} frame {n}");
            assert_eq!(
                rate.time_to_frame(t, Rounding::Floor),
                n,
                "{rate} frame {n}"
            );
            assert_eq!(
                rate.time_to_frame(t, Rounding::Nearest),
                n,
                "{rate} frame {n}"
            );
            let before = Time::from_flicks(t.flicks() - 1).unwrap();
            assert_eq!(rate.time_to_frame(before, Rounding::Floor), n - 1);
            assert_eq!(rate.time_to_frame(before, Rounding::Nearest), n);
            let just_below_half = Time::from_flicks(t.flicks() + half - 1).unwrap();
            assert_eq!(rate.time_to_frame(just_below_half, Rounding::Nearest), n);
            let half_way = Time::from_flicks(t.flicks() + half).unwrap();
            assert_eq!(rate.time_to_frame(half_way, Rounding::Nearest), n + 1);
            assert_eq!(rate.snap_to_frame(half_way, Rounding::Floor), Ok(t));
        }
    }
}

#[test]
fn samples_round_trip_exactly_for_the_first_million_samples() {
    for (rate, per_sample) in SAMPLE_TABLE {
        for n in 0..FRAMES {
            let t = rate.samples_to_time(n).unwrap();
            assert_eq!(t.flicks(), n * per_sample, "{rate} sample {n}");
            assert_eq!(rate.time_to_samples(t, Rounding::Floor), n);
            assert_eq!(rate.time_to_samples(t, Rounding::Nearest), n);
            let before = Time::from_flicks(t.flicks() - 1).unwrap();
            assert_eq!(rate.time_to_samples(before, Rounding::Floor), n - 1);
        }
    }
}

#[test]
fn negative_frames_mirror_positive_ones() {
    for (rate, per_frame) in FRAME_TABLE {
        for n in [-1, -2, -1000, -123_456] {
            let t = rate.frame_to_time(n).unwrap();
            assert_eq!(t.flicks(), n * per_frame);
            assert_eq!(rate.time_to_frame(t, Rounding::Floor), n);
        }
    }
}

#[test]
fn overflow_returns_err_and_never_wraps() {
    for (rate, per_frame) in FRAME_TABLE {
        assert!(matches!(
            rate.frame_to_time(i64::MAX),
            Err(TimeError::OutOfRange { .. })
        ));
        assert!(matches!(
            rate.frame_to_time(i64::MIN),
            Err(TimeError::OutOfRange { .. })
        ));
        let last = MAX_SAFE_FLICKS / per_frame;
        assert!(rate.frame_to_time(last).is_ok(), "{rate}");
        assert!(rate.frame_to_time(last + 1).is_err(), "{rate}");
        assert!(rate.frame_to_time(-last).is_ok(), "{rate}");
        assert!(rate.frame_to_time(-last - 1).is_err(), "{rate}");
        // The other direction never fails, even at the extremes.
        assert_eq!(rate.time_to_frame(Time::MAX, Rounding::Floor), last);
        assert_eq!(rate.time_to_frame(Time::MIN, Rounding::Floor), -last - 1);
    }
    for (rate, _) in SAMPLE_TABLE {
        assert!(rate.samples_to_time(i64::MAX).is_err());
        assert!(rate.samples_to_time(i64::MIN).is_err());
    }
    assert!(Time::from_flicks(i64::MAX).is_err());
    assert!(Time::from_flicks(i64::MIN).is_err());
    assert!(Duration::from_flicks(i64::MAX).is_err());
    assert!((Time::MAX + Duration::from_flicks(1).unwrap()).is_err());
    assert!((Time::MIN - Duration::from_flicks(1).unwrap()).is_err());
    assert!(Duration::MAX.checked_mul(i64::MAX).is_err());
    assert!((Duration::MAX + Duration::MAX).is_err());
    assert!(serde_json::from_str::<Time>(&i64::MAX.to_string()).is_err());
    assert!(serde_json::from_str::<Time>(&u64::MAX.to_string()).is_err());
    assert!("9223372036854775807".parse::<Time>().is_err());
    assert!("9223372036854775807s".parse::<Time>().is_err());
    assert!(
        "f:9223372036854775807"
            .parse::<cutweave_model::TimeInput>()
            .is_err()
    );
}

#[test]
fn ntsc_timecode_minutes_are_exact() {
    // 30000/1001: 1800 frames is 60.06 s exactly = 42,378,336,000 flicks.
    let t = FrameRate::FPS_29_97.frame_to_time(1800).unwrap();
    assert_eq!(t.flicks(), 42_378_336_000);
    assert_eq!(t.flicks() * 1000, 60_060 * FLICKS_PER_SECOND);
    // 1001 seconds of 23.976 is exactly 24,000 frames.
    let t = Time::from_secs(1001).unwrap();
    assert_eq!(
        FrameRate::FPS_23_976.time_to_frame(t, Rounding::Floor),
        24_000
    );
    assert_eq!(
        FrameRate::FPS_59_94.time_to_frame(t, Rounding::Floor),
        60_000
    );
}
