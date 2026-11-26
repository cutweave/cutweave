// SPDX-License-Identifier: GPL-3.0-or-later
//! Segment list → mixer plan: groups [`AudioSegment`]s into per-track buses and places every
//! clip on the output sample grid.
//!
//! **Sample grid.** A clip covering `[dst_start, dst_start + dst_duration)` plays the device
//! samples `[time_to_samples(dst_start), time_to_samples(dst_end))`, both ends rounded to the
//! nearest sample ([`Rounding::Nearest`]), so abutting clips tile the grid with no gap and no
//! overlap. The clip's first sample plays its source at `src_start`; sample `k` of the clip plays
//! source time `src_start + samples_to_time(k)`.

use std::sync::Arc;

use cutweave_model::{Duration, ItemId, MediaId, Rounding, SampleRate, Time, TrackId};
use cutweave_render::AudioSegment;

use crate::automation::Envelope;
use crate::dsp::db_to_gain;
use crate::error::MixError;

/// One clip on the output sample grid.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClipPlan {
    /// The clip.
    pub item: ItemId,
    /// Its media.
    pub media: MediaId,
    /// Its audio stream.
    pub stream: Option<u32>,
    /// First output sample (inclusive).
    pub start: i64,
    /// End output sample (exclusive).
    pub end: i64,
    /// Source time at `start`.
    pub src_start: Time,
    /// End of the played source range (exclusive).
    pub src_end: Time,
    /// Gain, pan and fades of the whole clip (WP-2E.01), on the output grid.
    pub env: Envelope,
}

/// The clips of one track, in start order, and the track's gain.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BusPlan {
    /// The track.
    pub track: TrackId,
    /// Linear track gain.
    pub gain: f32,
    /// Clips sorted by `start`. Shared with the lane renderers.
    pub clips: Arc<[ClipPlan]>,
}

impl BusPlan {
    /// First output sample with audio, and the end of the last clip.
    pub fn span(&self) -> Option<(i64, i64)> {
        let start = self.clips.first()?.start;
        let end = self.clips.iter().map(|c| c.end).max()?;
        Some((start, end))
    }
}

/// Builds the buses of `segments` at the output rate `rate`, in the order tracks first appear
/// (the `evaluate_audio` order: top to bottom).
///
/// Empty or sub-sample segments are dropped. Speed changes and reverse are rejected (P2).
pub(crate) fn plan_buses(
    segments: &[AudioSegment],
    rate: SampleRate,
) -> Result<Vec<BusPlan>, MixError> {
    let mut buses: Vec<(TrackId, f32, Vec<ClipPlan>)> = Vec::new();
    for seg in segments {
        if seg.reverse {
            return Err(MixError::Unsupported {
                item: seg.item,
                reason: "reverse playback of audio is not supported yet",
            });
        }
        if seg.speed.0 != seg.speed.1 || seg.speed.1 == 0 {
            return Err(MixError::Unsupported {
                item: seg.item,
                reason: "audio speed changes are not supported yet",
            });
        }
        let dst_end = seg.dst_start.checked_add(seg.dst_duration)?;
        let start = rate.time_to_samples(seg.dst_start, Rounding::Nearest);
        let end = rate.time_to_samples(dst_end, Rounding::Nearest);
        let index = if let Some(i) = buses.iter().position(|b| b.0 == seg.track) {
            i
        } else {
            buses.push((seg.track, db_to_gain(seg.track_gain_db), Vec::new()));
            buses.len() - 1
        };
        let bus = &mut buses[index];
        if end <= start {
            continue;
        }
        bus.2.push(ClipPlan {
            item: seg.item,
            media: seg.media,
            stream: seg.stream,
            start,
            end,
            src_start: seg.src_start,
            src_end: seg.src_start.checked_add(seg.dst_duration)?,
            env: Envelope::of_segment(seg, rate)?,
        });
    }
    Ok(buses
        .into_iter()
        .filter(|b| !b.2.is_empty())
        .map(|(track, gain, mut clips)| {
            clips.sort_by_key(|c| (c.start, c.end));
            BusPlan {
                track,
                gain,
                clips: clips.into(),
            }
        })
        .collect())
}

/// Source time played at output sample `at` of `clip` (`at ≥ clip.start`).
pub(crate) fn source_time_at(
    clip: &ClipPlan,
    rate: SampleRate,
    at: i64,
) -> Result<Time, cutweave_model::TimeError> {
    let offset = rate.samples_to_time(at - clip.start)?;
    clip.src_start
        .checked_add(Duration::from_flicks(offset.flicks())?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp, clippy::unwrap_used)]

    use super::*;

    fn seg(track: u128, item: u128, start: i64, dur: i64) -> AudioSegment {
        AudioSegment {
            item: ItemId::from_u128(item),
            track: TrackId::from_u128(track),
            media: MediaId::from_u128(1),
            stream: Some(0),
            dst_start: Time::from_flicks(start).unwrap(),
            dst_duration: Duration::from_flicks(dur).unwrap(),
            src_start: Time::from_flicks(0).unwrap(),
            clip_gain_db: 0.0,
            clip_start: Time::from_flicks(start).unwrap(),
            clip_duration: Duration::from_flicks(dur).unwrap(),
            gain: cutweave_model::Animatable::Static(0.0),
            pan: cutweave_model::Animatable::Static(0.0),
            fade_in: cutweave_model::Fade::default(),
            fade_out: cutweave_model::Fade::default(),
            track_gain_db: -6.0,
            reverse: false,
            speed: (1, 1),
            sample_rate: SampleRate::HZ_48_000,
        }
    }

    /// Flicks per sample at 48 kHz.
    const S: i64 = 14_700;

    #[test]
    fn clips_round_to_the_nearest_sample_and_tile() {
        let rate = SampleRate::HZ_48_000;
        // 29.97 fps frame boundaries are not sample-aligned: 1601.6 samples per frame.
        let frame = 23_543_520; // flicks per frame at 30000/1001
        let segs = [seg(1, 1, 0, frame), seg(1, 2, frame, frame)];
        let buses = plan_buses(&segs, rate).unwrap();
        assert_eq!(buses.len(), 1);
        let c = &buses[0].clips;
        assert_eq!((c[0].start, c[0].end), (0, 1602));
        assert_eq!((c[1].start, c[1].end), (1602, 3203));
        assert!((buses[0].gain - db_to_gain(-6.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn buses_follow_track_order_and_clips_sort_by_start() {
        let rate = SampleRate::HZ_48_000;
        let segs = [
            seg(2, 1, 10 * S, 5 * S),
            seg(1, 2, 0, 5 * S),
            seg(2, 3, 0, 5 * S),
            seg(3, 4, 0, S / 4), // shorter than half a sample: dropped
        ];
        let buses = plan_buses(&segs, rate).unwrap();
        assert_eq!(buses.len(), 2);
        assert_eq!(buses[0].track, TrackId::from_u128(2));
        assert_eq!(buses[0].clips[0].item, ItemId::from_u128(3));
        assert_eq!(buses[0].span(), Some((0, 15)));
        assert_eq!(buses[1].track, TrackId::from_u128(1));
    }

    #[test]
    fn speed_and_reverse_are_rejected() {
        let rate = SampleRate::HZ_48_000;
        let mut s = seg(1, 1, 0, S);
        s.reverse = true;
        assert!(matches!(
            plan_buses(&[s], rate),
            Err(MixError::Unsupported { .. })
        ));
        let mut s = seg(1, 1, 0, S);
        s.speed = (2, 1);
        assert!(plan_buses(&[s.clone()], rate).is_err());
        s.speed = (3, 3);
        assert!(plan_buses(&[s], rate).is_ok());
    }

    #[test]
    fn source_time_follows_the_output_grid() {
        let rate = SampleRate::HZ_48_000;
        let mut s = seg(1, 1, 100 * S, 1000 * S);
        s.src_start = Time::from_flicks(7 * S).unwrap();
        let buses = plan_buses(&[s], rate).unwrap();
        let clip = &buses[0].clips[0];
        assert_eq!(
            source_time_at(clip, rate, 100).unwrap(),
            Time::from_flicks(7 * S).unwrap()
        );
        assert_eq!(
            source_time_at(clip, rate, 150).unwrap(),
            Time::from_flicks(57 * S).unwrap()
        );
    }
}
