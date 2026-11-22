// SPDX-License-Identifier: GPL-3.0-or-later
//! Property tests over the `testing` strategies (WP-1A.01): frame ↔ time ↔ timecode, text and
//! serde round trips, checked arithmetic and ID order.
//!
//! They live in the crate (not `tests/`) because integration tests would need a self
//! dev-dependency with the `testing` feature, which the dependency-rule check rejects.

use proptest::prelude::*;

use crate::{
    Duration, EntityId, IdGen, ItemId, MAX_SAFE_FLICKS, Rounding, SampleRate, SeededGen, Time,
    TimeInput, Timecode, Uuid7Gen,
    testing::{
        arb_any_frame_rate, arb_duration, arb_frame_rate, arb_sample_rate, arb_time, arb_timecode,
    },
};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn frame_time_frame_is_identity(rate in arb_any_frame_rate(), frame in -10_000_000_i64..10_000_000) {
        if let Ok(t) = rate.frame_to_time(frame) {
            prop_assert_eq!(rate.time_to_frame(t, Rounding::Floor), frame);
            prop_assert_eq!(rate.time_to_frame(t, Rounding::Nearest), frame);
            prop_assert_eq!(rate.snap_to_frame(t, Rounding::Floor), Ok(t));
        }
    }

    #[test]
    fn snapping_lands_on_the_grid(rate in arb_frame_rate(), t in arb_time()) {
        let floor = rate.snap_to_frame(t, Rounding::Floor).unwrap();
        let period = rate.flicks_per_frame().unwrap();
        prop_assert!(floor <= t);
        prop_assert!(t.flicks() - floor.flicks() < period);
        prop_assert_eq!(floor.flicks().rem_euclid(period), 0);
        if let Ok(nearest) = rate.snap_to_frame(t, Rounding::Nearest) {
            prop_assert!((t.flicks() - nearest.flicks()).abs() <= period / 2);
            prop_assert_eq!(rate.snap_to_frame(nearest, Rounding::Nearest), Ok(nearest));
        }
    }

    #[test]
    fn sample_time_sample_is_identity(rate in arb_sample_rate(), n in -1_000_000_000_i64..1_000_000_000) {
        let t = rate.samples_to_time(n).unwrap();
        prop_assert_eq!(rate.time_to_samples(t, Rounding::Floor), n);
        prop_assert_eq!(rate.time_to_samples(t, Rounding::Nearest), n);
    }

    #[test]
    fn any_sample_rate_round_trips(hz in 1..=384_000_u32, n in -1_000_000_i64..1_000_000) {
        let rate = SampleRate::new(hz).unwrap();
        let t = rate.samples_to_time(n).unwrap();
        prop_assert_eq!(rate.time_to_samples(t, Rounding::Floor), n);
    }

    #[test]
    fn timecode_format_parse_is_identity(
        (rate, drop_frame, (frame, tc)) in arb_frame_rate()
            .prop_flat_map(|rate| any::<bool>().prop_map(move |df| (rate, df)))
            .prop_flat_map(|(rate, df)| (Just(rate), Just(df), arb_timecode(rate, df)))
    ) {
        let text = tc.to_string();
        let parsed: Timecode = text.parse().unwrap();
        prop_assert_eq!(parsed, tc);
        prop_assert_eq!(parsed.to_frame(rate), Ok(frame));
        prop_assert_eq!(parsed.is_drop_frame(), drop_frame && rate.drop_frames_per_minute().is_some());
        let t = parsed.to_time(rate).unwrap();
        prop_assert_eq!(Timecode::from_time(t, rate, parsed.is_drop_frame(), Rounding::Floor), Ok(tc));
        let input: TimeInput = text.parse().unwrap();
        prop_assert_eq!(input.resolve(Some(rate)), Ok(t));
    }

    #[test]
    fn time_text_round_trips(t in arb_time()) {
        prop_assert_eq!(t.to_string().parse::<Time>(), Ok(t));
        let input = TimeInput::from(t);
        prop_assert_eq!(input.to_string().parse::<TimeInput>(), Ok(input));
    }

    #[test]
    fn seconds_text_is_exact_to_the_flick(whole in 0_i64..1_000_000, micros in 0_i64..1_000_000) {
        let text = format!("{whole}.{micros:06}s");
        let t: Time = text.parse().unwrap();
        // micros * 705.6 flicks, rounded half up.
        let expected = whole * 705_600_000 + (micros * 7056 + 5) / 10;
        prop_assert_eq!(t.flicks(), expected);
    }

    #[test]
    fn serde_round_trips(t in arb_time(), d in arb_duration(), frame in 0_i64..1_000_000) {
        let json = serde_json::to_string(&t).unwrap();
        prop_assert_eq!(json.clone(), t.flicks().to_string());
        prop_assert_eq!(serde_json::from_str::<Time>(&json).unwrap(), t);
        prop_assert_eq!(serde_json::from_str::<Duration>(&serde_json::to_string(&d).unwrap()).unwrap(), d);
        for input in [TimeInput::Absolute(t), TimeInput::Frame(frame)] {
            let json = serde_json::to_string(&input).unwrap();
            prop_assert_eq!(serde_json::from_str::<TimeInput>(&json).unwrap(), input);
        }
    }

    #[test]
    fn unsafe_integers_are_rejected(v in prop_oneof![
        (MAX_SAFE_FLICKS + 1)..=i64::MAX,
        i64::MIN..=(-MAX_SAFE_FLICKS - 1),
    ]) {
        prop_assert!(serde_json::from_str::<Time>(&v.to_string()).is_err());
        prop_assert!(serde_json::from_str::<TimeInput>(&v.to_string()).is_err());
        prop_assert!(Time::from_flicks(v).is_err());
    }

    #[test]
    fn checked_arithmetic_matches_wide_math(a in arb_time(), d in arb_duration()) {
        let wide = i128::from(a.flicks()) + i128::from(d.flicks());
        let in_range = wide.abs() <= i128::from(MAX_SAFE_FLICKS);
        match a + d {
            Ok(sum) => { prop_assert!(in_range); prop_assert_eq!(i128::from(sum.flicks()), wide); }
            Err(_) => prop_assert!(!in_range),
        }
        let sat = a.saturating_add(d);
        prop_assert!(sat >= a);
        prop_assert!(sat <= Time::MAX);
        if let Ok(sum) = a + d {
            prop_assert_eq!(sum - a, Ok(d));
            prop_assert_eq!(sum - d, Ok(a));
        }
    }

    #[test]
    fn seeded_ids_order_by_creation(seed in any::<u64>(), n in 2_usize..300) {
        let mut generator = SeededGen::new(seed);
        let ids: Vec<ItemId> = (0..n).map(|_| generator.mint()).collect();
        prop_assert!(ids.windows(2).all(|w| w[0] < w[1]));
        let texts: Vec<String> = ids.iter().map(ToString::to_string).collect();
        prop_assert!(texts.windows(2).all(|w| w[0] < w[1]), "string order matches");
        for (id, text) in ids.iter().zip(&texts) {
            prop_assert_eq!(text.parse::<ItemId>(), Ok(*id));
        }
    }

    #[test]
    fn real_ids_order_by_creation(n in 2_usize..500) {
        let mut generator = Uuid7Gen;
        let ids: Vec<ItemId> = (0..n).map(|_| ItemId::new_with(&mut generator)).collect();
        prop_assert!(ids.windows(2).all(|w| w[0] < w[1]));
        let mut boxed: Box<dyn IdGen> = Box::new(Uuid7Gen);
        let later = ItemId::new_with(&mut boxed);
        prop_assert!(ids.iter().all(|id| *id < later));
    }
}
