//! Flicks time (arch §4.1, ADR-004) and its wire format (ADR-015).
//!
//! - In memory: `i64` flicks (1/705,600,000 s).
//! - On the wire: a JSON **integer**, validated as a JS safe integer (|t| <= 2^53 - 1).
//! - Inputs also accept timecode strings: `"HH:MM:SS:FF"`, `"62.5s"` and `"f:1500"`. Frame-based
//!   forms need a frame rate, which the engine supplies through [`with_frame_rate`] while
//!   deserializing params (the sequence rate).
//! - TypeScript: `number` (specta type override on the newtype).

use std::{borrow::Cow, cell::Cell, fmt};

use schemars::{JsonSchema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::wire;

/// Flicks per second: every common video frame rate and audio sample rate divides it exactly.
pub const FLICKS_PER_SECOND: i64 = 705_600_000;

/// Largest magnitude accepted on the wire: `Number.MAX_SAFE_INTEGER` (2^53 - 1), ~147 days.
pub const MAX_SAFE_FLICKS: i64 = (1 << 53) - 1;

/// Regex (for JSON Schema) of the accepted timecode string forms.
pub const TIMECODE_PATTERN: &str = r"^(\d+:[0-5]\d:[0-5]\d:\d+|\d+(\.\d+)?s|f:\d+)$";

/// A timeline time or duration in flicks (1/705,600,000 s). JSON integer on the wire; TS `number`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
pub struct Time(#[specta(type = specta_typescript::Number)] i64);

impl Time {
    /// Zero.
    pub const ZERO: Time = Time(0);

    /// Wraps raw flicks. Internal arithmetic is unchecked; the wire boundary validates.
    #[must_use]
    pub const fn from_flicks(flicks: i64) -> Time {
        Time(flicks)
    }

    /// Validated constructor used at the wire boundary.
    pub fn from_flicks_checked(flicks: i64) -> Result<Time, TimeError> {
        if flicks.abs() > MAX_SAFE_FLICKS {
            Err(TimeError::OutOfRange(flicks.to_string()))
        } else {
            Ok(Time(flicks))
        }
    }

    /// Whole seconds.
    #[must_use]
    pub const fn from_secs(secs: i64) -> Time {
        Time(secs * FLICKS_PER_SECOND)
    }

    /// Raw flicks.
    #[must_use]
    pub const fn flicks(self) -> i64 {
        self.0
    }

    /// Seconds as `f64` (display only, never for math).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / FLICKS_PER_SECOND as f64
    }

    /// Parses a timecode string (see module docs). `rate` is required for frame-based forms.
    pub fn parse(input: &str, rate: Option<FrameRate>) -> Result<Time, TimeError> {
        parse_timecode(input.trim(), rate)
    }
}

impl std::ops::Add for Time {
    type Output = Time;
    fn add(self, rhs: Time) -> Time {
        Time(self.0 + rhs.0)
    }
}

impl std::ops::Sub for Time {
    type Output = Time;
    fn sub(self, rhs: Time) -> Time {
        Time(self.0 - rhs.0)
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}fl ({:.3}s)", self.0, self.as_secs_f64())
    }
}

/// A rational frame rate (e.g. 30000/1001).
#[wire]
#[derive(Copy, Eq, Hash)]
pub struct FrameRate {
    /// Numerator (frames).
    pub num: u32,
    /// Denominator (seconds).
    pub den: u32,
}

impl FrameRate {
    /// Builds a rate; both parts must be non-zero (checked by [`FrameRate::validate`]).
    #[must_use]
    pub const fn new(num: u32, den: u32) -> FrameRate {
        FrameRate { num, den }
    }

    /// Rejects zero numerators/denominators.
    pub fn validate(self) -> Result<(), String> {
        if self.num == 0 || self.den == 0 {
            Err(format!(
                "frame rate {}/{} must be non-zero",
                self.num, self.den
            ))
        } else {
            Ok(())
        }
    }

    /// Nominal integer fps used by non-drop timecode labels (29.97 -> 30, 23.976 -> 24).
    #[must_use]
    pub fn nominal_fps(self) -> i64 {
        i64::from(self.num.div_ceil(self.den))
    }

    /// Time at the start of `frame` (floor for rates that do not divide flicks exactly).
    #[must_use]
    pub fn frame_to_time(self, frame: i64) -> Time {
        let n = i128::from(frame) * i128::from(FLICKS_PER_SECOND) * i128::from(self.den);
        Time(clamp_i64(n.div_euclid(i128::from(self.num))))
    }

    /// Frame containing `t` (floor).
    #[must_use]
    pub fn time_to_frame_floor(self, t: Time) -> i64 {
        let n = i128::from(t.0) * i128::from(self.num);
        clamp_i64(n.div_euclid(i128::from(FLICKS_PER_SECOND) * i128::from(self.den)))
    }

    /// Nearest frame boundary to `t` (ties round up).
    #[must_use]
    pub fn time_to_frame_nearest(self, t: Time) -> i64 {
        let d = i128::from(FLICKS_PER_SECOND) * i128::from(self.den);
        let n = 2 * i128::from(t.0) * i128::from(self.num) + d;
        clamp_i64(n.div_euclid(2 * d))
    }

    /// Snaps `t` to the nearest frame boundary.
    #[must_use]
    pub fn snap_to_frame(self, t: Time) -> Time {
        self.frame_to_time(self.time_to_frame_nearest(t))
    }
}

fn clamp_i64(v: i128) -> i64 {
    i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
}

/// Why a time value was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    /// Outside +-(2^53 - 1) flicks.
    #[error("time {0} is outside the safe-integer range of +-(2^53-1) flicks")]
    OutOfRange(String),
    /// Not an integer number of flicks.
    #[error("time must be an integer number of flicks, got {0}")]
    NotInteger(String),
    /// Frame-based timecode without a frame-rate context.
    #[error("timecode `{0}` needs a frame rate, but none is in context")]
    NeedsFrameRate(String),
    /// Drop-frame timecode is not supported by the spike.
    #[error("drop-frame timecode `{0}` is not supported (use non-drop `HH:MM:SS:FF`)")]
    DropFrame(String),
    /// Unparseable string.
    #[error("invalid timecode `{0}`; expected flicks, \"HH:MM:SS:FF\", \"62.5s\" or \"f:1500\"")]
    Syntax(String),
}

fn parse_digits(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn parse_timecode(s: &str, rate: Option<FrameRate>) -> Result<Time, TimeError> {
    let syntax = || TimeError::Syntax(s.to_owned());
    let need_rate = || TimeError::NeedsFrameRate(s.to_owned());
    let flicks: i128 = if let Some(frames) = s.strip_prefix("f:") {
        let frames = parse_digits(frames).ok_or_else(syntax)?;
        i128::from(rate.ok_or_else(need_rate)?.frame_to_time(frames).0)
    } else if let Some(secs) = s.strip_suffix('s') {
        let (whole, frac) = secs.split_once('.').unwrap_or((secs, ""));
        let whole = parse_digits(whole).ok_or_else(syntax)?;
        let mut total = i128::from(whole) * i128::from(FLICKS_PER_SECOND);
        if !frac.is_empty() {
            if frac.len() > 9 {
                return Err(syntax());
            }
            let digits = i128::from(parse_digits(frac).ok_or_else(syntax)?);
            let scale = 10_i128.pow(u32::try_from(frac.len()).map_err(|_| syntax())?);
            // Round half up to the nearest flick; exact for <= 5 decimal places.
            total += (2 * digits * i128::from(FLICKS_PER_SECOND) + scale) / (2 * scale);
        }
        total
    } else if s.contains(';') {
        return Err(TimeError::DropFrame(s.to_owned()));
    } else {
        let parts: Vec<&str> = s.split(':').collect();
        let [h, m, sec, ff] = parts.as_slice() else {
            return Err(syntax());
        };
        let (h, m, sec, ff) = (
            parse_digits(h).ok_or_else(syntax)?,
            parse_digits(m).ok_or_else(syntax)?,
            parse_digits(sec).ok_or_else(syntax)?,
            parse_digits(ff).ok_or_else(syntax)?,
        );
        let rate = rate.ok_or_else(need_rate)?;
        if m >= 60 || sec >= 60 || ff >= rate.nominal_fps() {
            return Err(syntax());
        }
        let frames = (h * 3600 + m * 60 + sec)
            .checked_mul(rate.nominal_fps())
            .and_then(|f| f.checked_add(ff))
            .ok_or_else(|| TimeError::OutOfRange(s.to_owned()))?;
        i128::from(rate.frame_to_time(frames).0)
    };
    let flicks = i64::try_from(flicks).map_err(|_| TimeError::OutOfRange(s.to_owned()))?;
    Time::from_flicks_checked(flicks)
}

thread_local! {
    static PARSE_RATE: Cell<Option<FrameRate>> = const { Cell::new(None) };
}

/// Runs `f` with `rate` as the frame-rate context for timecode strings deserialized on this
/// thread (restores the previous context afterwards, also on unwind).
pub fn with_frame_rate<R>(rate: FrameRate, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<FrameRate>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PARSE_RATE.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(PARSE_RATE.with(|c| c.replace(Some(rate))));
    f()
}

impl Serialize for Time {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Time, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = Time;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("integer flicks or a timecode string")
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Time, E> {
                Time::from_flicks_checked(v).map_err(E::custom)
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Time, E> {
                let v = i64::try_from(v)
                    .map_err(|_| E::custom(TimeError::OutOfRange(v.to_string())))?;
                self.visit_i64(v)
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Time, E> {
                // JSON like `1e3` or `2.0` arrives as f64; accept only exact safe integers.
                if !v.is_finite() || v.fract() != 0.0 {
                    return Err(E::custom(TimeError::NotInteger(v.to_string())));
                }
                if v.abs() > MAX_SAFE_FLICKS as f64 {
                    return Err(E::custom(TimeError::OutOfRange(v.to_string())));
                }
                Ok(Time(v as i64))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Time, E> {
                let rate = PARSE_RATE.with(Cell::get);
                Time::parse(v, rate).map_err(E::custom)
            }
        }
        d.deserialize_any(V)
    }
}

impl JsonSchema for Time {
    fn schema_name() -> Cow<'static, str> {
        "Time".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(generator: &mut SchemaGenerator) -> schemars::Schema {
        // schemars 1.x knows the direction: inputs widen to timecode strings, outputs are integers.
        if generator.contract().is_deserialize() {
            json_schema!({
                "description": "Flicks (1/705600000 s) as an integer, or a timecode string: \"HH:MM:SS:FF\" (sequence rate), \"62.5s\" or \"f:1500\" (frame number).",
                "anyOf": [
                    { "type": "integer", "minimum": -MAX_SAFE_FLICKS, "maximum": MAX_SAFE_FLICKS },
                    { "type": "string", "pattern": TIMECODE_PATTERN }
                ]
            })
        } else {
            json_schema!({
                "description": "Flicks (1/705600000 s).",
                "type": "integer",
                "minimum": -MAX_SAFE_FLICKS,
                "maximum": MAX_SAFE_FLICKS
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R30: FrameRate = FrameRate::new(30, 1);
    const NTSC: FrameRate = FrameRate::new(30_000, 1001);

    #[test]
    fn common_rates_divide_flicks_exactly() {
        for (num, den) in [
            (24, 1),
            (25, 1),
            (30, 1),
            (50, 1),
            (60, 1),
            (120, 1),
            (24_000, 1001),
            (30_000, 1001),
            (60_000, 1001),
        ] {
            let r = FrameRate::new(num, den);
            let t = r.frame_to_time(1);
            assert_eq!(r.frame_to_time(1000), Time(t.0 * 1000), "{num}/{den}");
            assert_eq!(r.time_to_frame_floor(r.frame_to_time(12_345)), 12_345);
        }
    }

    #[test]
    fn timecode_forms() {
        assert_eq!(
            Time::parse("00:00:02:00", Some(R30)),
            Ok(Time::from_secs(2))
        );
        assert_eq!(
            Time::parse("00:01:02:03", Some(R30)),
            Ok(Time::from_secs(62) + R30.frame_to_time(3))
        );
        assert_eq!(
            Time::parse("62.5s", None),
            Ok(Time(62 * FLICKS_PER_SECOND + FLICKS_PER_SECOND / 2))
        );
        assert_eq!(Time::parse("2s", None), Ok(Time::from_secs(2)));
        assert_eq!(Time::parse("f:60", Some(R30)), Ok(Time::from_secs(2)));
        assert_eq!(
            Time::parse("f:30000", Some(NTSC)),
            Ok(Time::from_secs(1001))
        );
        assert!(matches!(
            Time::parse("f:60", None),
            Err(TimeError::NeedsFrameRate(_))
        ));
        assert!(matches!(
            Time::parse("00:00:01;02", Some(NTSC)),
            Err(TimeError::DropFrame(_))
        ));
        assert!(matches!(
            Time::parse("00:00:01:30", Some(R30)),
            Err(TimeError::Syntax(_))
        ));
        assert!(matches!(
            Time::parse("1.5", None),
            Err(TimeError::Syntax(_))
        ));
        assert!(matches!(
            Time::parse("-2s", None),
            Err(TimeError::Syntax(_))
        ));
    }

    #[test]
    fn wire_is_integer_and_safe_range_is_enforced() {
        assert_eq!(serde_json::to_string(&Time(42)).ok().as_deref(), Some("42"));
        let max = serde_json::json!(MAX_SAFE_FLICKS);
        assert_eq!(
            serde_json::from_value::<Time>(max).ok(),
            Some(Time(MAX_SAFE_FLICKS))
        );
        let over = serde_json::json!(MAX_SAFE_FLICKS + 1);
        assert!(serde_json::from_value::<Time>(over).is_err());
        assert!(serde_json::from_value::<Time>(serde_json::json!(2.5)).is_err());
        assert_eq!(
            serde_json::from_value::<Time>(serde_json::json!(3.0)).ok(),
            Some(Time(3))
        );
        let tc = with_frame_rate(R30, || {
            serde_json::from_value::<Time>(serde_json::json!("00:00:01:15"))
        });
        assert_eq!(tc.ok(), Some(Time(FLICKS_PER_SECOND * 3 / 2)));
    }

    #[test]
    fn frame_rate_context_is_scoped() {
        with_frame_rate(R30, || {
            assert_eq!(PARSE_RATE.with(Cell::get), Some(R30));
        });
        assert_eq!(PARSE_RATE.with(Cell::get), None);
    }

    #[test]
    fn nearest_and_snap() {
        let half = Time(R30.frame_to_time(1).0 / 2);
        assert_eq!(R30.time_to_frame_nearest(half), 1);
        assert_eq!(R30.time_to_frame_nearest(Time(half.0 - 1)), 0);
        assert_eq!(R30.snap_to_frame(Time(half.0 - 1)), Time::ZERO);
    }
}
