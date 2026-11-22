// SPDX-License-Identifier: GPL-3.0-or-later
//! Proptest strategies for model types (feature `testing`). Reused by the property-test harness
//! of `cutweave-ops` (WP-1B.07).

use proptest::prelude::*;

use crate::{Duration, FrameRate, MAX_SAFE_FLICKS, SampleRate, Time, Timecode};

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
