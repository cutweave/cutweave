// SPDX-License-Identifier: GPL-3.0-or-later
//! SMPTE timecode labels and the [`TimeInput`] wire type for time arguments (arch §4.1,
//! ADR-0015, the S5 decision).
//!
//! - [`Timecode`] is a label `HH:MM:SS:FF` (non-drop) or `HH:MM:SS;FF` (drop-frame, 29.97 and
//!   59.94 fps only) that maps to a frame index at a given [`FrameRate`]. Non-drop timecode
//!   counts frames at the nominal integer rate (30 for 29.97), as every NLE does.
//! - [`TimeInput`] is what command arguments accept: integer flicks, seconds (`"62.5s"`), a
//!   frame (`"f:1500"`) or a timecode. Frame-based forms stay unresolved until
//!   [`TimeInput::resolve`] is given the frame rate of the sequence that owns the command's
//!   target, at validation time. Commands without a sequence context pass `None` and so accept
//!   only flicks and seconds.
//!
//! ```
//! use cutweave_model::{FrameRate, Time, TimeInput, Timecode};
//!
//! let at: TimeInput = serde_json::from_str(r#""00:00:02:00""#)?;
//! assert!(at.needs_frame_rate());
//! assert_eq!(at.resolve(Some(FrameRate::FPS_30))?, Time::from_secs(2)?);
//! assert!(at.resolve(None).is_err()); // no sequence context: flicks or seconds only
//!
//! let tc: Timecode = "00:01:00;02".parse()?; // drop-frame
//! assert_eq!(tc.to_frame(FrameRate::FPS_29_97)?, 1800);
//! assert_eq!(Timecode::from_frame(1800, FrameRate::FPS_29_97, true)?.to_string(), "00:01:00;02");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::{borrow::Cow, fmt, str::FromStr};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{
    rate::{FrameRate, Rounding},
    time::{MAX_SAFE_FLICKS, Time, TimeError, parse_absolute, parse_digits},
};

/// JSON Schema pattern of every string form a [`TimeInput`] accepts.
pub const TIME_INPUT_PATTERN: &str =
    r"^-?[0-9]+(\.[0-9]+)?s$|^-?[0-9]+$|^f:[0-9]+$|^[0-9]+[:;][0-9]+[:;][0-9]+[:;][0-9]+$";

/// A SMPTE timecode label: `HH:MM:SS:FF`, or drop-frame `HH:MM:SS;FF`.
///
/// A label is only meaningful together with a [`FrameRate`]: [`Timecode::to_frame`] checks the
/// frame field against the rate and rejects drop-frame labels that do not exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Timecode {
    hours: u32,
    minutes: u8,
    seconds: u8,
    frames: u32,
    drop_frame: bool,
}

impl Timecode {
    /// A label from its fields; `Err` if minutes or seconds are 60 or more. The frame field is
    /// checked against a rate by [`Timecode::to_frame`].
    pub fn new(
        hours: u32,
        minutes: u8,
        seconds: u8,
        frames: u32,
        drop_frame: bool,
    ) -> Result<Timecode, TimeError> {
        let tc = Timecode {
            hours,
            minutes,
            seconds,
            frames,
            drop_frame,
        };
        if minutes >= 60 || seconds >= 60 {
            return Err(tc.invalid("minutes and seconds must be below 60"));
        }
        Ok(tc)
    }

    /// Hours (unbounded; long durations count past 24).
    #[must_use]
    pub const fn hours(self) -> u32 {
        self.hours
    }

    /// Minutes, `0..60`.
    #[must_use]
    pub const fn minutes(self) -> u8 {
        self.minutes
    }

    /// Seconds, `0..60`.
    #[must_use]
    pub const fn seconds(self) -> u8 {
        self.seconds
    }

    /// Frames within the second.
    #[must_use]
    pub const fn frames(self) -> u32 {
        self.frames
    }

    /// Whether this is a drop-frame label (`;` before the frames).
    #[must_use]
    pub const fn is_drop_frame(self) -> bool {
        self.drop_frame
    }

    fn invalid(self, reason: &str) -> TimeError {
        TimeError::InvalidTimecode {
            input: self.to_string(),
            reason: reason.to_owned(),
        }
    }

    /// The label of `frame` (≥ 0) at `rate`; `drop_frame` selects drop-frame counting, which
    /// only 29.97 and 59.94 fps have.
    pub fn from_frame(
        frame: i64,
        rate: FrameRate,
        drop_frame: bool,
    ) -> Result<Timecode, TimeError> {
        if frame < 0 {
            return Err(TimeError::Negative {
                what: "timecode frame",
                value: frame.to_string(),
            });
        }
        let nominal = i128::from(rate.nominal_fps());
        let frame_wide = i128::from(frame);
        let mut label = frame_wide;
        if drop_frame {
            let drop = i128::from(drop_frames(rate)?);
            let per_minute = nominal * 60 - drop;
            let per_ten_minutes = nominal * 600 - drop * 9;
            let tens = frame_wide / per_ten_minutes;
            let rest = frame_wide % per_ten_minutes;
            let skipped_minutes = if rest > drop {
                (rest - drop) / per_minute
            } else {
                0
            };
            label = frame_wide + drop * 9 * tens + drop * skipped_minutes;
        }
        let total_seconds = label / nominal;
        let hours = u32::try_from(total_seconds / 3600)
            .map_err(|_| TimeError::out_of_range(format_args!("frame {frame}")))?;
        // The remainders are below 60 and below `nominal` (a u32): the casts are exact.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(Timecode {
            hours,
            minutes: ((total_seconds / 60) % 60) as u8,
            seconds: (total_seconds % 60) as u8,
            frames: (label % nominal) as u32,
            drop_frame,
        })
    }

    /// The label of time `t` (≥ 0) at `rate`, after mapping `t` to a frame with `rounding`.
    pub fn from_time(
        t: Time,
        rate: FrameRate,
        drop_frame: bool,
        rounding: Rounding,
    ) -> Result<Timecode, TimeError> {
        Timecode::from_frame(rate.time_to_frame(t, rounding), rate, drop_frame)
    }

    /// The frame index this label names at `rate`.
    ///
    /// `Err` if the frame field is not below the nominal rate, if a drop-frame label is used at
    /// a rate without drop-frame, or if it names a dropped label (`00:01:00;00` at 29.97).
    pub fn to_frame(self, rate: FrameRate) -> Result<i64, TimeError> {
        let nominal = i128::from(rate.nominal_fps());
        if i128::from(self.frames) >= nominal {
            return Err(self.invalid(&format!("frames must be below {nominal} at {rate} fps")));
        }
        let total_minutes = i128::from(self.hours) * 60 + i128::from(self.minutes);
        let mut frame =
            (total_minutes * 60 + i128::from(self.seconds)) * nominal + i128::from(self.frames);
        if self.drop_frame {
            let drop = i128::from(drop_frames(rate)?);
            if self.seconds == 0
                && i128::from(self.frames) < drop
                && !self.minutes.is_multiple_of(10)
            {
                return Err(self.invalid("this drop-frame label is skipped (it does not exist)"));
            }
            frame -= drop * (total_minutes - total_minutes / 10);
        }
        i64::try_from(frame).map_err(|_| TimeError::out_of_range(self))
    }

    /// The start time of the frame this label names at `rate`.
    pub fn to_time(self, rate: FrameRate) -> Result<Time, TimeError> {
        rate.frame_to_time(self.to_frame(rate)?)
    }
}

fn drop_frames(rate: FrameRate) -> Result<u32, TimeError> {
    rate.drop_frames_per_minute()
        .ok_or(TimeError::DropFrameUnsupported {
            num: rate.num(),
            den: rate.den(),
        })
}

/// `01:02:03:04`, or `01:02:03;04` for drop-frame. Hours have at least two digits.
impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(
            f,
            "{:02}:{:02}:{:02}{sep}{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

/// Parses `H:MM:SS:FF` (any number of digits per field) with `:` or `;` separators; a `;`
/// before the frames marks drop-frame.
impl FromStr for Timecode {
    type Err = TimeError;
    fn from_str(s: &str) -> Result<Timecode, TimeError> {
        let s = s.trim();
        let syntax = || TimeError::Syntax(s.to_owned());
        let fields: Vec<&str> = s.split([':', ';']).collect();
        let [h, m, sec, ff] = fields.as_slice() else {
            return Err(syntax());
        };
        let drop_frame = s.as_bytes().get(s.len() - ff.len() - 1) == Some(&b';');
        let number = |field: &str| parse_digits(field).ok_or_else(syntax);
        let field = |v: i128| u32::try_from(v).map_err(|_| TimeError::out_of_range(s));
        let minutes = u8::try_from(number(m)?).map_err(|_| syntax())?;
        let seconds = u8::try_from(number(sec)?).map_err(|_| syntax())?;
        Timecode::new(
            field(number(h)?)?,
            minutes,
            seconds,
            field(number(ff)?)?,
            drop_frame,
        )
    }
}

/// A time argument as clients send it (arch §4.1): flicks, seconds, a frame or a timecode.
///
/// Integers and seconds resolve on their own; frames and timecodes need the frame rate of the
/// sequence that owns the command's target ([`TimeInput::resolve`]). On the wire it is a JSON
/// integer or one of the strings `"62.5s"`, `"123"`, `"f:1500"`, `"00:01:02:03"`,
/// `"00:01:02;03"`; in TypeScript `number | string`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, specta::Type)]
#[specta(type = TimeInputWire)]
pub enum TimeInput {
    /// Flicks or seconds: no frame rate needed.
    Absolute(Time),
    /// `"f:N"`: the start of frame `N` (≥ 0) at the sequence rate.
    Frame(i64),
    /// A timecode label at the sequence rate.
    Timecode(Timecode),
}

/// TypeScript shape of [`TimeInput`]: `number | string`. Only its type definition is used.
#[derive(Serialize, specta::Type)]
#[serde(untagged)]
#[specta(inline)]
#[allow(dead_code)]
enum TimeInputWire {
    Flicks(#[specta(type = specta_typescript::Number)] i64),
    Text(String),
}

impl TimeInput {
    /// Whether resolving needs a frame rate (frames and timecodes).
    #[must_use]
    pub const fn needs_frame_rate(self) -> bool {
        !matches!(self, TimeInput::Absolute(_))
    }

    /// The time this input names. `rate` is the frame rate of the sequence that owns the
    /// target; `None` accepts only flicks and seconds.
    pub fn resolve(self, rate: Option<FrameRate>) -> Result<Time, TimeError> {
        match (self, rate) {
            (TimeInput::Absolute(t), _) => Ok(t),
            (TimeInput::Frame(n), Some(rate)) => rate.frame_to_time(n),
            (TimeInput::Timecode(tc), Some(rate)) => tc.to_time(rate),
            (other, None) => Err(TimeError::NeedsFrameRate(other.to_string())),
        }
    }
}

impl From<Time> for TimeInput {
    fn from(t: Time) -> TimeInput {
        TimeInput::Absolute(t)
    }
}

/// The same text the input was parsed from, in canonical form.
impl fmt::Display for TimeInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeInput::Absolute(t) => write!(f, "{t}"),
            TimeInput::Frame(n) => write!(f, "f:{n}"),
            TimeInput::Timecode(tc) => write!(f, "{tc}"),
        }
    }
}

impl FromStr for TimeInput {
    type Err = TimeError;
    fn from_str(s: &str) -> Result<TimeInput, TimeError> {
        let s = s.trim();
        if let Some(n) = s.strip_prefix("f:") {
            let n = parse_digits(n).ok_or_else(|| TimeError::Syntax(s.to_owned()))?;
            if n > MAX_SAFE_FLICKS.into() {
                return Err(TimeError::out_of_range(s));
            }
            return i64::try_from(n)
                .map(TimeInput::Frame)
                .map_err(|_| TimeError::out_of_range(s));
        }
        if s.contains([':', ';']) {
            return s.parse().map(TimeInput::Timecode);
        }
        match parse_absolute(s)? {
            Some(t) => Ok(TimeInput::Absolute(t)),
            None => Err(TimeError::Syntax(s.to_owned())),
        }
    }
}

/// Flicks as a JSON integer; frames and timecodes as their strings.
impl Serialize for TimeInput {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            TimeInput::Absolute(t) => t.serialize(s),
            other => s.collect_str(other),
        }
    }
}

impl<'de> Deserialize<'de> for TimeInput {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<TimeInput, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = TimeInput;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("integer flicks or a string: \"62.5s\", \"f:1500\" or \"00:01:02:03\"")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<TimeInput, E> {
                Time::from_flicks(v)
                    .map(TimeInput::Absolute)
                    .map_err(E::custom)
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<TimeInput, E> {
                let v = i64::try_from(v).map_err(|_| E::custom(TimeError::out_of_range(v)))?;
                self.visit_i64(v)
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<TimeInput, E> {
                Time::deserialize(de::value::F64Deserializer::<E>::new(v)).map(TimeInput::Absolute)
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<TimeInput, E> {
                v.parse().map_err(E::custom)
            }
        }
        d.deserialize_any(V)
    }
}

impl JsonSchema for TimeInput {
    fn schema_name() -> Cow<'static, str> {
        "TimeInput".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A time: integer flicks (1/705600000 s), or a string: seconds (\"62.5s\"), a frame (\"f:1500\") or a timecode (\"00:01:02:03\", drop-frame \"00:01:02;03\"). Frames and timecodes use the frame rate of the target's sequence.",
            "anyOf": [
                { "type": "integer", "minimum": -MAX_SAFE_FLICKS, "maximum": MAX_SAFE_FLICKS },
                { "type": "string", "pattern": TIME_INPUT_PATTERN }
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R30: FrameRate = FrameRate::FPS_30;
    const DF30: FrameRate = FrameRate::FPS_29_97;
    const DF60: FrameRate = FrameRate::FPS_59_94;

    fn tc(s: &str) -> Timecode {
        s.parse().unwrap()
    }

    #[test]
    fn non_drop_labels() {
        assert_eq!(tc("00:01:02:03").to_frame(R30).unwrap(), 62 * 30 + 3);
        assert_eq!(
            Timecode::from_frame(62 * 30 + 3, R30, false).unwrap(),
            tc("00:01:02:03")
        );
        // NTSC non-drop counts at the nominal 30.
        assert_eq!(tc("00:01:00:00").to_frame(DF30).unwrap(), 1800);
        assert_eq!(tc("1:2:3:4").to_string(), "01:02:03:04");
        assert_eq!(tc("123:00:00:00").to_string(), "123:00:00:00");
        assert_eq!(
            Timecode::from_frame(119, FrameRate::FPS_120, false)
                .unwrap()
                .to_string(),
            "00:00:00:119"
        );
        assert!(tc("00:00:00:30").to_frame(R30).is_err());
        assert!(tc("00:00:00:24").to_frame(FrameRate::FPS_23_976).is_err());
        assert!("00:60:00:00".parse::<Timecode>().is_err());
        assert!("00:00:60:00".parse::<Timecode>().is_err());
    }

    #[test]
    fn drop_frame_labels() {
        // Well-known 29.97 DF anchors.
        for (frame, label) in [
            (0, "00:00:00;00"),
            (1799, "00:00:59;29"),
            (1800, "00:01:00;02"),
            (3597, "00:01:59;29"),
            (3598, "00:02:00;02"),
            (17_982, "00:10:00;00"),
            (107_892, "01:00:00;00"),
        ] {
            assert_eq!(
                Timecode::from_frame(frame, DF30, true).unwrap().to_string(),
                label
            );
            assert_eq!(tc(label).to_frame(DF30).unwrap(), frame, "{label}");
        }
        // 59.94 DF drops 4 labels per minute.
        assert_eq!(
            Timecode::from_frame(3600, DF60, true).unwrap().to_string(),
            "00:01:00;04"
        );
        assert_eq!(tc("01:00:00;00").to_frame(DF60).unwrap(), 215_784);
        // Dropped labels do not exist; tenth minutes keep them.
        assert!(tc("00:01:00;00").to_frame(DF30).is_err());
        assert!(tc("00:01:00;01").to_frame(DF30).is_err());
        assert!(tc("00:01:00;03").to_frame(DF60).is_err());
        assert!(tc("00:10:00;00").to_frame(DF30).is_ok());
        // Drop-frame needs an NTSC rate.
        assert!(matches!(
            tc("00:00:01;00").to_frame(R30),
            Err(TimeError::DropFrameUnsupported { .. })
        ));
        assert!(Timecode::from_frame(1, FrameRate::FPS_23_976, true).is_err());
        assert!(Timecode::from_frame(-1, DF30, false).is_err());
    }

    #[test]
    fn real_time_of_an_ntsc_hour() {
        // One hour of 29.97 DF timecode is 107,892 frames = 3599.9964 s (drop-frame tracks
        // the wall clock to within 3.6 ms per hour).
        let t = tc("01:00:00;00").to_time(DF30).unwrap();
        assert_eq!(t.flicks(), 107_892 * 23_543_520);
    }

    #[test]
    fn huge_frames_do_not_overflow() {
        assert!(Timecode::from_frame(i64::MAX, FrameRate::new(1, 1000).unwrap(), false).is_err());
        assert!(Timecode::from_frame(i64::MAX, DF30, true).is_err());
        let big = Timecode::from_frame(i64::from(u32::MAX) * 30, R30, false).unwrap();
        assert_eq!(big.to_frame(R30).unwrap(), i64::from(u32::MAX) * 30);
        assert!(tc("4294967295:59:59:29").to_time(R30).is_err());
        assert!("99999999999:00:00:00".parse::<Timecode>().is_err());
    }

    #[test]
    fn time_input_forms() {
        let parse = |s: &str| s.parse::<TimeInput>().unwrap();
        assert_eq!(
            parse("62.5s").resolve(None).unwrap().flicks(),
            44_100_000_000
        );
        assert_eq!(parse("123").resolve(None).unwrap().flicks(), 123);
        assert_eq!(parse("-2s").resolve(None).unwrap().flicks(), -1_411_200_000);
        assert_eq!(
            parse("f:1500").resolve(Some(R30)).unwrap(),
            Time::from_secs(50).unwrap()
        );
        assert_eq!(
            parse("00:00:02:00").resolve(Some(R30)).unwrap(),
            Time::from_secs(2).unwrap()
        );
        assert_eq!(
            parse("00:00:01;00").resolve(Some(DF30)).unwrap(),
            DF30.frame_to_time(30).unwrap()
        );
        assert!(matches!(
            parse("f:1").resolve(None),
            Err(TimeError::NeedsFrameRate(_))
        ));
        for bad in [
            "f:",
            "f:-1",
            "f:1.5",
            "1.5",
            "x",
            "00:00:00",
            "00:00:00:00:00",
            "f:9007199254740992",
        ] {
            assert!(bad.parse::<TimeInput>().is_err(), "{bad:?}");
        }
        for text in [
            "62.5s",
            "29400000",
            "f:1500",
            "00:01:02:03",
            "00:01:02;03",
            "-0.5s",
        ] {
            assert_eq!(parse(text).to_string(), text);
        }
    }

    #[test]
    fn time_input_wire() {
        let v: TimeInput = serde_json::from_str("42").unwrap();
        assert_eq!(v, TimeInput::Absolute(Time::from_flicks(42).unwrap()));
        assert_eq!(serde_json::to_string(&v).unwrap(), "42");
        let v: TimeInput = serde_json::from_str(r#""00:00:01:00""#).unwrap();
        assert_eq!(serde_json::to_string(&v).unwrap(), r#""00:00:01:00""#);
        let v: TimeInput = serde_json::from_str(r#""1.5s""#).unwrap();
        assert_eq!(serde_json::to_string(&v).unwrap(), "1058400000");
        assert!(serde_json::from_str::<TimeInput>("9007199254740992").is_err());
        assert!(serde_json::from_str::<TimeInput>("0.5").is_err());
        assert!(serde_json::from_str::<TimeInput>("2.0").is_ok());
        assert!(serde_json::from_str::<TimeInput>("[]").is_err());
    }
}
