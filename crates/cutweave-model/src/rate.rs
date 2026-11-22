// SPDX-License-Identifier: GPL-3.0-or-later
//! Frame rates, sample rates and the only allowed conversions between frames/samples and
//! [`Time`] (arch §4.1, ADR-0004).
//!
//! All math is integer (`i128` intermediates), never floating point. For the rates in the
//! ADR-0004 table the conversions are exact. For any other rate a unit `n` starts at the first
//! flick at or after its exact rational start (`ceil(n · 705,600,000 · den / num)`), so
//! `time_to_frame(frame_to_time(n), _) == n` still holds for every rate the type accepts.

use std::{borrow::Cow, fmt};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::time::{FLICKS_PER_SECOND, Time, TimeError, safe_flicks};

/// How a time that falls between two frame (or sample) boundaries is mapped to a unit index.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    /// The unit that contains the time (the latest boundary at or before it).
    Floor,
    /// The nearest boundary; exact halves round up (towards the later unit).
    Nearest,
}

const F: i128 = FLICKS_PER_SECOND as i128;

/// `ceil(a / b)` for `b > 0`.
fn div_ceil(a: i128, b: i128) -> i128 {
    -(-a).div_euclid(b)
}

/// Start time of unit `n` at `num/den` units per second (ceil to whole flicks).
fn unit_to_time(n: i64, num: u32, den: u32) -> Result<Time, TimeError> {
    let flicks = div_ceil(i128::from(n) * F * i128::from(den), i128::from(num));
    safe_flicks(flicks)
        .and_then(Time::from_flicks)
        .map_err(|_| TimeError::out_of_range(format_args!("unit {n} at {num}/{den} per second")))
}

/// Unit index of `t` at `num/den` units per second.
fn time_to_unit(t: Time, num: u32, den: u32, rounding: Rounding) -> i64 {
    let period_den = F * i128::from(den);
    let scaled = i128::from(t.flicks()) * i128::from(num);
    let unit = match rounding {
        Rounding::Floor => scaled.div_euclid(period_den),
        Rounding::Nearest => (2 * scaled + period_den).div_euclid(2 * period_den),
    };
    // |t| < 2^53 and the period is at least 2 flicks, so |unit| < 2^53: always fits.
    i64::try_from(unit).unwrap_or(if unit < 0 { i64::MIN } else { i64::MAX })
}

fn is_exact(num: u32, den: u32) -> bool {
    (F * i128::from(den)) % i128::from(num) == 0
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// A video frame rate as an exact rational `num/den` frames per second (`30000/1001` is
/// 29.97 fps), always in lowest terms.
///
/// Serialized as `{"num": 30000, "den": 1001}`; unreduced input is reduced on parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(try_from = "RawFrameRate")]
// The validating `try_from` does not change the shape; specta only needs the fields.
#[specta(skip_attr = "serde")]
pub struct FrameRate {
    num: u32,
    den: u32,
}

/// The unvalidated wire form of a [`FrameRate`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFrameRate {
    num: u32,
    den: u32,
}

impl TryFrom<RawFrameRate> for FrameRate {
    type Error = TimeError;
    fn try_from(raw: RawFrameRate) -> Result<FrameRate, TimeError> {
        FrameRate::new(raw.num, raw.den)
    }
}

impl FrameRate {
    /// 23.976 fps (24000/1001, NTSC film).
    pub const FPS_23_976: FrameRate = FrameRate::reduced(24_000, 1001);
    /// 24 fps.
    pub const FPS_24: FrameRate = FrameRate::reduced(24, 1);
    /// 25 fps (PAL).
    pub const FPS_25: FrameRate = FrameRate::reduced(25, 1);
    /// 29.97 fps (30000/1001, NTSC).
    pub const FPS_29_97: FrameRate = FrameRate::reduced(30_000, 1001);
    /// 30 fps.
    pub const FPS_30: FrameRate = FrameRate::reduced(30, 1);
    /// 50 fps.
    pub const FPS_50: FrameRate = FrameRate::reduced(50, 1);
    /// 59.94 fps (60000/1001).
    pub const FPS_59_94: FrameRate = FrameRate::reduced(60_000, 1001);
    /// 60 fps.
    pub const FPS_60: FrameRate = FrameRate::reduced(60, 1);
    /// 120 fps.
    pub const FPS_120: FrameRate = FrameRate::reduced(120, 1);

    /// The rates of the ADR-0004 table, all exact in flicks.
    pub const COMMON: [FrameRate; 9] = [
        FrameRate::FPS_23_976,
        FrameRate::FPS_24,
        FrameRate::FPS_25,
        FrameRate::FPS_29_97,
        FrameRate::FPS_30,
        FrameRate::FPS_50,
        FrameRate::FPS_59_94,
        FrameRate::FPS_60,
        FrameRate::FPS_120,
    ];

    /// For constants that are already in lowest terms.
    const fn reduced(num: u32, den: u32) -> FrameRate {
        FrameRate { num, den }
    }

    /// `num/den` frames per second, reduced to lowest terms.
    ///
    /// `Err` if either part is zero or a frame would be shorter than 2 flicks (above
    /// 352,800,000 fps), which keeps every conversion round-trip exact.
    pub fn new(num: u32, den: u32) -> Result<FrameRate, TimeError> {
        if num == 0 || den == 0 || i128::from(num) * 2 > F * i128::from(den) {
            return Err(TimeError::InvalidFrameRate { num, den });
        }
        let g = gcd(num, den);
        Ok(FrameRate {
            num: num / g,
            den: den / g,
        })
    }

    /// Frames (numerator).
    #[must_use]
    pub const fn num(self) -> u32 {
        self.num
    }

    /// Seconds (denominator).
    #[must_use]
    pub const fn den(self) -> u32 {
        self.den
    }

    /// Frames per second as `f64`, for display only.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    /// Whether a frame is a whole number of flicks (true for every rate in [`Self::COMMON`]).
    #[must_use]
    pub fn is_exact(self) -> bool {
        is_exact(self.num, self.den)
    }

    /// Flicks per frame, if [`exact`](Self::is_exact).
    #[must_use]
    pub fn flicks_per_frame(self) -> Option<i64> {
        if self.is_exact() {
            i64::try_from(F * i128::from(self.den) / i128::from(self.num)).ok()
        } else {
            None
        }
    }

    /// The start time of `frame` (frame 0 starts at zero). `Err` if it is outside the safe
    /// time range.
    pub fn frame_to_time(self, frame: i64) -> Result<Time, TimeError> {
        unit_to_time(frame, self.num, self.den)
    }

    /// The frame index of `t`: the frame containing it ([`Rounding::Floor`]) or the nearest
    /// frame boundary ([`Rounding::Nearest`]). Never fails.
    #[must_use]
    pub fn time_to_frame(self, t: Time, rounding: Rounding) -> i64 {
        time_to_unit(t, self.num, self.den, rounding)
    }

    /// `t` moved onto the frame grid. `Err` only if the snapped time leaves the safe range.
    pub fn snap_to_frame(self, t: Time, rounding: Rounding) -> Result<Time, TimeError> {
        self.frame_to_time(self.time_to_frame(t, rounding))
    }

    /// Whole frames per second used for timecode labels: the rate rounded to the nearest
    /// integer (30 for 29.97, 24 for 23.976), at least 1.
    #[must_use]
    pub fn nominal_fps(self) -> u32 {
        ((self.num + self.den / 2) / self.den).max(1)
    }

    /// Frame numbers skipped per minute (except every tenth) by drop-frame timecode: 2 at
    /// 29.97 fps, 4 at 59.94 fps, `None` for rates without drop-frame timecode.
    #[must_use]
    pub fn drop_frames_per_minute(self) -> Option<u32> {
        match (self.num, self.den) {
            (30_000, 1001) => Some(2),
            (60_000, 1001) => Some(4),
            _ => None,
        }
    }
}

/// `30000/1001`, or `30` when the denominator is 1.
impl fmt::Display for FrameRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl JsonSchema for FrameRate {
    fn schema_name() -> Cow<'static, str> {
        "FrameRate".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A frame rate as an exact fraction of frames per second, e.g. {\"num\": 30000, \"den\": 1001} for 29.97 fps.",
            "type": "object",
            "properties": {
                "num": { "type": "integer", "minimum": 1, "maximum": u32::MAX, "description": "Frames." },
                "den": { "type": "integer", "minimum": 1, "maximum": u32::MAX, "description": "Seconds." }
            },
            "required": ["num", "den"],
            "additionalProperties": false
        })
    }
}

/// An audio sample rate in Hz.
///
/// Serialized as a JSON integer (`48000`).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
#[serde(try_from = "u32", into = "u32")]
#[specta(skip_attr = "serde")]
pub struct SampleRate(u32);

impl SampleRate {
    /// 44.1 kHz.
    pub const HZ_44_100: SampleRate = SampleRate(44_100);
    /// 48 kHz.
    pub const HZ_48_000: SampleRate = SampleRate(48_000);
    /// 96 kHz.
    pub const HZ_96_000: SampleRate = SampleRate(96_000);

    /// The rates of the ADR-0004 table, all exact in flicks.
    pub const COMMON: [SampleRate; 3] = [
        SampleRate::HZ_44_100,
        SampleRate::HZ_48_000,
        SampleRate::HZ_96_000,
    ];

    /// `hz` samples per second; `Err` if zero or above 352.8 MHz (a sample must be at least
    /// 2 flicks).
    pub fn new(hz: u32) -> Result<SampleRate, TimeError> {
        if hz == 0 || i128::from(hz) * 2 > F {
            Err(TimeError::InvalidSampleRate(hz))
        } else {
            Ok(SampleRate(hz))
        }
    }

    /// Samples per second.
    #[must_use]
    pub const fn hz(self) -> u32 {
        self.0
    }

    /// Whether a sample is a whole number of flicks (true for every rate in [`Self::COMMON`]).
    #[must_use]
    pub fn is_exact(self) -> bool {
        is_exact(self.0, 1)
    }

    /// Flicks per sample, if [`exact`](Self::is_exact).
    #[must_use]
    pub fn flicks_per_sample(self) -> Option<i64> {
        self.is_exact()
            .then(|| FLICKS_PER_SECOND / i64::from(self.0))
    }

    /// The start time of sample `n`. `Err` if it is outside the safe time range.
    pub fn samples_to_time(self, n: i64) -> Result<Time, TimeError> {
        unit_to_time(n, self.0, 1)
    }

    /// The sample index of `t` (see [`Rounding`]). Never fails.
    #[must_use]
    pub fn time_to_samples(self, t: Time, rounding: Rounding) -> i64 {
        time_to_unit(t, self.0, 1, rounding)
    }
}

impl TryFrom<u32> for SampleRate {
    type Error = TimeError;
    fn try_from(hz: u32) -> Result<SampleRate, TimeError> {
        SampleRate::new(hz)
    }
}

impl From<SampleRate> for u32 {
    fn from(rate: SampleRate) -> u32 {
        rate.0
    }
}

/// `48000 Hz`.
impl fmt::Display for SampleRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} Hz", self.0)
    }
}

impl JsonSchema for SampleRate {
    fn schema_name() -> Cow<'static, str> {
        "SampleRate".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "Audio samples per second (Hz).",
            "type": "integer",
            "minimum": 1,
            "maximum": FLICKS_PER_SECOND / 2
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_validate_and_reduce() {
        assert_eq!(FrameRate::new(60_000, 2002), Ok(FrameRate::FPS_29_97));
        assert_eq!(FrameRate::new(48, 2), Ok(FrameRate::FPS_24));
        assert!(FrameRate::new(0, 1).is_err());
        assert!(FrameRate::new(1, 0).is_err());
        assert!(FrameRate::new(u32::MAX, 1).is_err());
        assert!(FrameRate::new(352_800_000, 1).is_ok());
        assert!(FrameRate::new(352_800_001, 1).is_err());
        assert!(SampleRate::new(0).is_err());
        assert!(SampleRate::new(u32::MAX).is_err());
        assert_eq!(SampleRate::new(48_000), Ok(SampleRate::HZ_48_000));
        for r in FrameRate::COMMON {
            assert_eq!(FrameRate::new(r.num(), r.den()), Ok(r), "{r} is reduced");
        }
    }

    #[test]
    fn display() {
        assert_eq!(FrameRate::FPS_29_97.to_string(), "30000/1001");
        assert_eq!(FrameRate::FPS_25.to_string(), "25");
        assert_eq!(SampleRate::HZ_48_000.to_string(), "48000 Hz");
    }

    #[test]
    fn nominal_and_drop_frame() {
        let nominal: Vec<u32> = FrameRate::COMMON.iter().map(|r| r.nominal_fps()).collect();
        assert_eq!(nominal, [24, 24, 25, 30, 30, 50, 60, 60, 120]);
        assert_eq!(FrameRate::FPS_29_97.drop_frames_per_minute(), Some(2));
        assert_eq!(FrameRate::FPS_59_94.drop_frames_per_minute(), Some(4));
        assert_eq!(FrameRate::FPS_23_976.drop_frames_per_minute(), None);
        assert_eq!(FrameRate::FPS_30.drop_frames_per_minute(), None);
        assert_eq!(FrameRate::new(1, 3).unwrap().nominal_fps(), 1);
    }

    #[test]
    fn rounding_modes() {
        let r = FrameRate::FPS_30; // 23,520,000 flicks per frame
        let at = |f: i64| Time::from_flicks(f).unwrap();
        assert_eq!(r.time_to_frame(at(23_519_999), Rounding::Floor), 0);
        assert_eq!(r.time_to_frame(at(23_520_000), Rounding::Floor), 1);
        assert_eq!(r.time_to_frame(at(11_759_999), Rounding::Nearest), 0);
        assert_eq!(r.time_to_frame(at(11_760_000), Rounding::Nearest), 1); // half rounds up
        assert_eq!(r.time_to_frame(at(-1), Rounding::Floor), -1);
        assert_eq!(r.time_to_frame(at(-11_760_000), Rounding::Nearest), 0);
        assert_eq!(r.time_to_frame(at(-11_760_001), Rounding::Nearest), -1);
        assert_eq!(
            r.snap_to_frame(at(30_000_000), Rounding::Nearest),
            Ok(at(23_520_000))
        );
        assert_eq!(
            r.snap_to_frame(at(40_000_000), Rounding::Nearest),
            Ok(at(47_040_000))
        );
        assert_eq!(
            r.snap_to_frame(at(40_000_000), Rounding::Floor),
            Ok(at(23_520_000))
        );
        assert!(r.snap_to_frame(Time::MAX, Rounding::Nearest).is_err());
        assert!(r.snap_to_frame(Time::MAX, Rounding::Floor).is_ok());
    }

    #[test]
    fn inexact_rates_start_units_at_the_ceiling_flick() {
        let r = FrameRate::new(11, 1).unwrap(); // 64,145,454.54… flicks per frame
        assert!(!r.is_exact());
        assert_eq!(r.flicks_per_frame(), None);
        assert_eq!(r.frame_to_time(1).unwrap().flicks(), 64_145_455);
        assert_eq!(r.frame_to_time(-1).unwrap().flicks(), -64_145_454);
        for n in -1000..1000 {
            let t = r.frame_to_time(n).unwrap();
            assert_eq!(r.time_to_frame(t, Rounding::Floor), n);
            assert_eq!(r.time_to_frame(t, Rounding::Nearest), n);
            let before = Time::from_flicks(t.flicks() - 1).unwrap();
            assert_eq!(r.time_to_frame(before, Rounding::Floor), n - 1);
        }
    }

    #[test]
    fn serde_forms() {
        let json = serde_json::to_string(&FrameRate::FPS_29_97).unwrap();
        assert_eq!(json, r#"{"num":30000,"den":1001}"#);
        let parsed: FrameRate = serde_json::from_str(r#"{"num":60000,"den":2002}"#).unwrap();
        assert_eq!(parsed, FrameRate::FPS_29_97);
        assert!(serde_json::from_str::<FrameRate>(r#"{"num":0,"den":1}"#).is_err());
        assert!(serde_json::from_str::<FrameRate>(r#"{"num":30,"den":1,"x":1}"#).is_err());
        assert_eq!(
            serde_json::to_string(&SampleRate::HZ_44_100).unwrap(),
            "44100"
        );
        assert!(serde_json::from_str::<SampleRate>("0").is_err());
        assert_eq!(
            serde_json::from_str::<Rounding>(r#""nearest""#).unwrap(),
            Rounding::Nearest
        );
    }
}
