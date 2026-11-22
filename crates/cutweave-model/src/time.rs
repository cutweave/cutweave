// SPDX-License-Identifier: GPL-3.0-or-later
//! Flicks time (arch §4.1, ADR-0004) and its wire format (ADR-0015).
//!
//! - In memory: [`Time`] (a signed position) and [`Duration`] (a non-negative span), both `i64`
//!   flicks (1/705,600,000 s). Every value is kept inside the JavaScript safe-integer range
//!   ±(2⁵³ − 1) flicks (about 147 days), so anything the engine holds can be written to JSON and
//!   read back losslessly by the engine and by TypeScript.
//! - Arithmetic is checked: `+`/`-` return `Result` and never wrap; the `saturating_*` methods
//!   are the explicit alternative.
//! - On the wire: a JSON **integer**; TypeScript `number` (specta type override). Deserialization
//!   also accepts the context-free string forms `"62.5s"` and `"123"` (raw flicks). Frame-based
//!   forms (`"00:01:02:03"`, `"f:1500"`) need a frame rate and go through
//!   [`TimeInput`](crate::TimeInput).
//! - Conversions to and from frames and samples live on [`FrameRate`](crate::FrameRate) and
//!   [`SampleRate`](crate::SampleRate); they are the only allowed ones.
//!
//! ```
//! use cutweave_model::{Duration, FrameRate, Rounding, Time};
//!
//! let fps = FrameRate::FPS_29_97;
//! let t = fps.frame_to_time(1800)?; // one NTSC "minute" of frames: 60.06 s
//! assert_eq!(fps.time_to_frame(t, Rounding::Floor), 1800);
//! assert_eq!((t + Duration::from_secs(1)?)?, Time::from_flicks(t.flicks() + 705_600_000)?);
//! assert!((Time::MAX + Duration::from_flicks(1)?).is_err()); // never wraps
//! assert_eq!(t.to_string(), "60.06s");
//! # Ok::<(), cutweave_model::TimeError>(())
//! ```

use std::{borrow::Cow, fmt, ops, str::FromStr};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Flicks per second. Every common frame rate (23.976 … 120 fps) and audio rate (44.1, 48 and
/// 96 kHz) divides it exactly (ADR-0004).
pub const FLICKS_PER_SECOND: i64 = 705_600_000;

/// Largest magnitude a [`Time`] or [`Duration`] may have: `Number.MAX_SAFE_INTEGER` (2⁵³ − 1)
/// flicks, about 147 days.
pub const MAX_SAFE_FLICKS: i64 = (1 << 53) - 1;

/// JSON Schema pattern of the context-free string forms of a time: raw flicks or seconds.
pub const ABSOLUTE_TIME_PATTERN: &str = r"^-?[0-9]+(\.[0-9]+)?s$|^-?[0-9]+$";

/// Most fractional digits accepted in a seconds string (`"1.123456789012345678s"`).
pub(crate) const MAX_SECONDS_DECIMALS: usize = 18;

/// Why a time value, rate or timecode was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TimeError {
    /// The result is outside ±(2⁵³ − 1) flicks (also every arithmetic overflow).
    #[error("time {value} is outside the safe range of ±(2^53-1) flicks (about 147 days)")]
    OutOfRange {
        /// The offending value or expression, for the message.
        value: String,
    },
    /// A duration (or a frame/sample count that must be non-negative) would be negative.
    #[error("{what} must not be negative, got {value}")]
    Negative {
        /// What was negative (`"duration"`, `"frame"`, ...).
        what: &'static str,
        /// The offending value.
        value: String,
    },
    /// A JSON number with a fractional part where integer flicks are required.
    #[error("time must be an integer number of flicks, got {0}")]
    NotInteger(String),
    /// A frame-based form (`"HH:MM:SS:FF"`, `"f:N"`) without a frame rate to resolve it.
    #[error(
        "`{0}` is frame-based and needs the sequence frame rate; use flicks or seconds (\"62.5s\") here"
    )]
    NeedsFrameRate(String),
    /// A frame rate with a zero numerator or denominator, or one so high that a frame would be
    /// shorter than 2 flicks.
    #[error(
        "invalid frame rate {num}/{den}: both parts must be non-zero and the rate at most 352800000 fps"
    )]
    InvalidFrameRate {
        /// Numerator.
        num: u32,
        /// Denominator.
        den: u32,
    },
    /// A zero sample rate.
    #[error("invalid sample rate {0} Hz: must be non-zero")]
    InvalidSampleRate(u32),
    /// Drop-frame timecode at a rate that has none (only 29.97 and 59.94 fps do).
    #[error("drop-frame timecode is only defined for 29.97 and 59.94 fps, not {num}/{den}")]
    DropFrameUnsupported {
        /// Rate numerator.
        num: u32,
        /// Rate denominator.
        den: u32,
    },
    /// A syntactically valid timecode whose fields are out of range at the given rate, or a
    /// drop-frame label that does not exist.
    #[error("invalid timecode `{input}`: {reason}")]
    InvalidTimecode {
        /// The timecode text.
        input: String,
        /// Why it is invalid.
        reason: String,
    },
    /// Not any accepted time syntax.
    #[error(
        "invalid time `{0}`; expected integer flicks, seconds (\"62.5s\"), a frame (\"f:1500\") or a timecode (\"00:01:02:03\", drop-frame \"00:01:02;03\")"
    )]
    Syntax(String),
}

impl TimeError {
    pub(crate) fn out_of_range(value: impl fmt::Display) -> Self {
        Self::OutOfRange {
            value: value.to_string(),
        }
    }
}

/// Checks that `v` fits the safe range and narrows it to `i64`.
pub(crate) fn safe_flicks(v: i128) -> Result<i64, TimeError> {
    if v.unsigned_abs() > MAX_SAFE_FLICKS.unsigned_abs().into() {
        Err(TimeError::out_of_range(v))
    } else {
        // In range of ±(2^53 - 1), so the conversion cannot fail.
        i64::try_from(v).map_err(|_| TimeError::out_of_range(v))
    }
}

/// A position on a timeline or in a source, in flicks (1/705,600,000 s).
///
/// Always within ±[`MAX_SAFE_FLICKS`]; may be negative (relative offsets). Serialized as a JSON
/// integer, exported to TypeScript as `number`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
pub struct Time(#[specta(type = specta_typescript::Number)] i64);

impl Time {
    /// Zero.
    pub const ZERO: Time = Time(0);
    /// The latest representable time, `+(2⁵³ − 1)` flicks.
    pub const MAX: Time = Time(MAX_SAFE_FLICKS);
    /// The earliest representable time, `-(2⁵³ − 1)` flicks.
    pub const MIN: Time = Time(-MAX_SAFE_FLICKS);

    /// A time of `flicks`; `Err` outside ±[`MAX_SAFE_FLICKS`].
    pub fn from_flicks(flicks: i64) -> Result<Time, TimeError> {
        safe_flicks(flicks.into()).map(Time)
    }

    /// A time of whole seconds.
    pub fn from_secs(secs: i64) -> Result<Time, TimeError> {
        safe_flicks(i128::from(secs) * i128::from(FLICKS_PER_SECOND)).map(Time)
    }

    /// The raw flicks.
    #[must_use]
    pub const fn flicks(self) -> i64 {
        self.0
    }

    /// Seconds as `f64`, for display and logging only; never use it for timeline math.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // |flicks| < 2^53: exact in f64.
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / FLICKS_PER_SECOND as f64
    }

    /// `self + d`, or `Err` if the result leaves the safe range.
    pub fn checked_add(self, d: Duration) -> Result<Time, TimeError> {
        safe_flicks(i128::from(self.0) + i128::from(d.0)).map(Time)
    }

    /// `self - d`, or `Err` if the result leaves the safe range.
    pub fn checked_sub(self, d: Duration) -> Result<Time, TimeError> {
        safe_flicks(i128::from(self.0) - i128::from(d.0)).map(Time)
    }

    /// `self + d`, clamped to [`Time::MAX`].
    #[must_use]
    pub fn saturating_add(self, d: Duration) -> Time {
        Time(self.0.saturating_add(d.0).min(MAX_SAFE_FLICKS))
    }

    /// `self - d`, clamped to [`Time::MIN`].
    #[must_use]
    pub fn saturating_sub(self, d: Duration) -> Time {
        Time(self.0.saturating_sub(d.0).max(-MAX_SAFE_FLICKS))
    }

    /// The span from `earlier` to `self`; `Err` if `earlier` is later than `self` or the span
    /// leaves the safe range.
    pub fn duration_since(self, earlier: Time) -> Result<Duration, TimeError> {
        let d = i128::from(self.0) - i128::from(earlier.0);
        if d < 0 {
            return Err(TimeError::Negative {
                what: "duration",
                value: d.to_string(),
            });
        }
        safe_flicks(d).map(Duration)
    }

    /// The absolute distance between two times; `Err` if it leaves the safe range.
    pub fn abs_diff(self, other: Time) -> Result<Duration, TimeError> {
        safe_flicks((i128::from(self.0) - i128::from(other.0)).abs()).map(Duration)
    }
}

/// A non-negative span of time in flicks (item durations, media durations).
///
/// Always within `0..=`[`MAX_SAFE_FLICKS`]. Serialized as a JSON integer, exported to TypeScript
/// as `number`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
pub struct Duration(#[specta(type = specta_typescript::Number)] i64);

impl Duration {
    /// Zero length.
    pub const ZERO: Duration = Duration(0);
    /// The longest representable duration, `2⁵³ − 1` flicks.
    pub const MAX: Duration = Duration(MAX_SAFE_FLICKS);

    /// A duration of `flicks`; `Err` if negative or above [`MAX_SAFE_FLICKS`].
    pub fn from_flicks(flicks: i64) -> Result<Duration, TimeError> {
        if flicks < 0 {
            return Err(TimeError::Negative {
                what: "duration",
                value: flicks.to_string(),
            });
        }
        safe_flicks(flicks.into()).map(Duration)
    }

    /// A duration of whole seconds.
    pub fn from_secs(secs: i64) -> Result<Duration, TimeError> {
        Duration::from_flicks(safe_flicks(
            i128::from(secs) * i128::from(FLICKS_PER_SECOND),
        )?)
    }

    /// The raw flicks.
    #[must_use]
    pub const fn flicks(self) -> i64 {
        self.0
    }

    /// Whether this is zero length.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Seconds as `f64`, for display and logging only.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // < 2^53: exact in f64.
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / FLICKS_PER_SECOND as f64
    }

    /// `self + other`, or `Err` above [`Duration::MAX`].
    pub fn checked_add(self, other: Duration) -> Result<Duration, TimeError> {
        safe_flicks(i128::from(self.0) + i128::from(other.0)).map(Duration)
    }

    /// `self - other`, or `Err` if negative.
    pub fn checked_sub(self, other: Duration) -> Result<Duration, TimeError> {
        Duration::from_flicks(self.0 - other.0)
    }

    /// `self + other`, clamped to [`Duration::MAX`].
    #[must_use]
    pub fn saturating_add(self, other: Duration) -> Duration {
        Duration(self.0.saturating_add(other.0).min(MAX_SAFE_FLICKS))
    }

    /// `self - other`, clamped to zero.
    #[must_use]
    pub fn saturating_sub(self, other: Duration) -> Duration {
        Duration((self.0 - other.0).max(0))
    }

    /// `self * n`, or `Err` if negative or above [`Duration::MAX`].
    pub fn checked_mul(self, n: i64) -> Result<Duration, TimeError> {
        let v = i128::from(self.0) * i128::from(n);
        if v < 0 {
            return Err(TimeError::Negative {
                what: "duration",
                value: v.to_string(),
            });
        }
        safe_flicks(v).map(Duration)
    }
}

impl From<Duration> for Time {
    fn from(d: Duration) -> Time {
        Time(d.0)
    }
}

impl TryFrom<Time> for Duration {
    type Error = TimeError;
    fn try_from(t: Time) -> Result<Duration, TimeError> {
        Duration::from_flicks(t.0)
    }
}

impl ops::Add<Duration> for Time {
    type Output = Result<Time, TimeError>;
    fn add(self, rhs: Duration) -> Self::Output {
        self.checked_add(rhs)
    }
}

impl ops::Sub<Duration> for Time {
    type Output = Result<Time, TimeError>;
    fn sub(self, rhs: Duration) -> Self::Output {
        self.checked_sub(rhs)
    }
}

impl ops::Sub<Time> for Time {
    type Output = Result<Duration, TimeError>;
    fn sub(self, rhs: Time) -> Self::Output {
        self.duration_since(rhs)
    }
}

impl ops::Add for Duration {
    type Output = Result<Duration, TimeError>;
    fn add(self, rhs: Duration) -> Self::Output {
        self.checked_add(rhs)
    }
}

impl ops::Sub for Duration {
    type Output = Result<Duration, TimeError>;
    fn sub(self, rhs: Duration) -> Self::Output {
        self.checked_sub(rhs)
    }
}

// ---------------------------------------------------------------------------------------------
// Text forms
// ---------------------------------------------------------------------------------------------

/// Splits an optional leading `-`.
pub(crate) fn split_sign(s: &str) -> (bool, &str) {
    match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    }
}

/// Parses a non-empty run of ASCII digits (no sign, no `+`).
pub(crate) fn parse_digits(s: &str) -> Option<i128> {
    if s.is_empty() || s.len() > 30 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Parses the context-free forms: raw flicks (`"-123"`) and seconds (`"62.5s"`, `"-1.25s"`).
/// Seconds are rounded to the nearest flick (ties away from zero); `Ok(None)` if `s` is not one
/// of these forms.
pub(crate) fn parse_absolute(s: &str) -> Result<Option<Time>, TimeError> {
    let syntax = || TimeError::Syntax(s.to_owned());
    let (negative, body) = split_sign(s);
    let magnitude: i128 = if let Some(secs) = body.strip_suffix('s') {
        let (whole, frac) = match secs.split_once('.') {
            Some((w, f)) => (w, Some(f)),
            None => (secs, None),
        };
        let whole = parse_digits(whole).ok_or_else(syntax)?;
        let mut total = whole
            .checked_mul(i128::from(FLICKS_PER_SECOND))
            .ok_or_else(|| TimeError::out_of_range(s))?;
        if let Some(frac) = frac {
            if frac.len() > MAX_SECONDS_DECIMALS {
                return Err(syntax());
            }
            let digits = parse_digits(frac).ok_or_else(syntax)?;
            let scale = 10_i128.pow(u32::try_from(frac.len()).map_err(|_| syntax())?);
            // Round half up on the magnitude: exact whenever the decimal is exact in flicks.
            total += (2 * digits * i128::from(FLICKS_PER_SECOND) + scale) / (2 * scale);
        }
        total
    } else if !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit()) {
        parse_digits(body).ok_or_else(|| TimeError::out_of_range(s))?
    } else {
        return Ok(None);
    };
    let flicks = if negative { -magnitude } else { magnitude };
    safe_flicks(flicks)
        .map(|f| Some(Time(f)))
        .map_err(|_| TimeError::out_of_range(s))
}

/// Writes `flicks` as exact seconds (`"62.5s"`) when the decimal expansion terminates within
/// nine places, otherwise as raw flicks (`"29400000"`). Both forms parse back exactly.
fn write_canonical(f: &mut fmt::Formatter<'_>, flicks: i64) -> fmt::Result {
    let magnitude = flicks.unsigned_abs();
    let fps = FLICKS_PER_SECOND.unsigned_abs();
    let whole = magnitude / fps;
    let rem = magnitude % fps;
    let sign = if flicks < 0 { "-" } else { "" };
    if rem == 0 {
        return write!(f, "{sign}{whole}s");
    }
    let scaled = u128::from(rem) * 1_000_000_000;
    if scaled % u128::from(fps) != 0 {
        return write!(f, "{flicks}");
    }
    let nanos = format!("{:09}", scaled / u128::from(fps));
    write!(f, "{sign}{whole}.{}s", nanos.trim_end_matches('0'))
}

/// Writes approximate seconds with `precision` decimals (`"1.042s"`), for humans.
#[allow(clippy::cast_precision_loss)] // |flicks| < 2^53: exact in f64.
fn write_approx(f: &mut fmt::Formatter<'_>, flicks: i64, precision: usize) -> fmt::Result {
    let secs = flicks as f64 / FLICKS_PER_SECOND as f64;
    write!(f, "{secs:.precision$}s")
}

/// `{}` writes the lossless canonical form (exact seconds such as `"62.5s"` when the value has
/// a terminating decimal, raw flicks otherwise) that [`FromStr`] reads back exactly; `{:.3}`
/// writes rounded seconds (`"0.042s"`) for humans.
impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match f.precision() {
            Some(p) => write_approx(f, self.0, p),
            None => write_canonical(f, self.0),
        }
    }
}

/// Same forms as [`Time`]'s `Display`.
impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match f.precision() {
            Some(p) => write_approx(f, self.0, p),
            None => write_canonical(f, self.0),
        }
    }
}

/// Parses raw flicks (`"123"`) or seconds (`"62.5s"`). Frame-based forms need a rate: use
/// [`TimeInput`](crate::TimeInput).
impl FromStr for Time {
    type Err = TimeError;
    fn from_str(s: &str) -> Result<Time, TimeError> {
        let s = s.trim();
        match parse_absolute(s)? {
            Some(t) => Ok(t),
            None if looks_frame_based(s) => Err(TimeError::NeedsFrameRate(s.to_owned())),
            None => Err(TimeError::Syntax(s.to_owned())),
        }
    }
}

/// Parses like [`Time`] and rejects negative values.
impl FromStr for Duration {
    type Err = TimeError;
    fn from_str(s: &str) -> Result<Duration, TimeError> {
        Duration::try_from(s.parse::<Time>()?)
    }
}

pub(crate) fn looks_frame_based(s: &str) -> bool {
    s.starts_with("f:") || s.contains(':') || s.contains(';')
}

// ---------------------------------------------------------------------------------------------
// Serde: JSON integer out; integer, integral float or context-free string in.
// ---------------------------------------------------------------------------------------------

impl Serialize for Time {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(self.0)
    }
}

impl Serialize for Duration {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(self.0)
    }
}

/// Accepts what a JSON client may send for flicks: an integer, an integral float (`2.0`,
/// `1e3`) or a context-free string. Everything is range-checked.
struct FlicksVisitor;

impl de::Visitor<'_> for FlicksVisitor {
    type Value = Time;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("integer flicks within ±(2^53-1), or a string such as \"62.5s\"")
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Time, E> {
        safe_flicks(v.into()).map(Time).map_err(E::custom)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Time, E> {
        safe_flicks(v.into()).map(Time).map_err(E::custom)
    }

    fn visit_i128<E: de::Error>(self, v: i128) -> Result<Time, E> {
        safe_flicks(v).map(Time).map_err(E::custom)
    }

    fn visit_u128<E: de::Error>(self, v: u128) -> Result<Time, E> {
        let v = i128::try_from(v).map_err(|_| E::custom(TimeError::out_of_range(v)))?;
        self.visit_i128(v)
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Time, E> {
        if !v.is_finite() || v.fract() != 0.0 {
            return Err(E::custom(TimeError::NotInteger(v.to_string())));
        }
        if v.abs() > MAX_SAFE_FLICKS as f64 {
            return Err(E::custom(TimeError::out_of_range(v)));
        }
        // Integral and |v| <= 2^53 - 1: the cast is exact.
        Ok(Time(v as i64))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Time, E> {
        v.parse().map_err(E::custom)
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Time, D::Error> {
        d.deserialize_any(FlicksVisitor)
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let t = d.deserialize_any(FlicksVisitor)?;
        Duration::try_from(t).map_err(de::Error::custom)
    }
}

fn flicks_schema(generator: &SchemaGenerator, minimum: i64, what: &str) -> Schema {
    // schemars 1.x knows the direction: inputs widen to context-free strings, outputs are
    // plain integers.
    if generator.contract().is_deserialize() {
        json_schema!({
            "description": format!(
                "{what} in flicks (1/705600000 s) as an integer, or a string: seconds (\"62.5s\") or raw flicks (\"123\")."
            ),
            "anyOf": [
                { "type": "integer", "minimum": minimum, "maximum": MAX_SAFE_FLICKS },
                { "type": "string", "pattern": ABSOLUTE_TIME_PATTERN }
            ]
        })
    } else {
        json_schema!({
            "description": format!("{what} in flicks (1/705600000 s)."),
            "type": "integer",
            "minimum": minimum,
            "maximum": MAX_SAFE_FLICKS
        })
    }
}

impl JsonSchema for Time {
    fn schema_name() -> Cow<'static, str> {
        "Time".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        flicks_schema(generator, -MAX_SAFE_FLICKS, "A time")
    }
}

impl JsonSchema for Duration {
    fn schema_name() -> Cow<'static, str> {
        "Duration".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        flicks_schema(generator, 0, "A non-negative duration")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(flicks: i64) -> Time {
        Time::from_flicks(flicks).unwrap()
    }

    fn d(flicks: i64) -> Duration {
        Duration::from_flicks(flicks).unwrap()
    }

    #[test]
    fn construction_enforces_the_safe_range() {
        assert_eq!(Time::from_flicks(MAX_SAFE_FLICKS), Ok(Time::MAX));
        assert_eq!(Time::from_flicks(-MAX_SAFE_FLICKS), Ok(Time::MIN));
        assert!(Time::from_flicks(MAX_SAFE_FLICKS + 1).is_err());
        assert!(Time::from_flicks(i64::MAX).is_err());
        assert!(Time::from_flicks(i64::MIN).is_err());
        assert!(Duration::from_flicks(-1).is_err());
        assert!(Duration::from_flicks(i64::MAX).is_err());
        assert!(Time::from_secs(i64::MAX).is_err());
        assert!(Duration::from_secs(i64::MIN).is_err());
        assert_eq!(Time::from_secs(2).unwrap().flicks(), 2 * FLICKS_PER_SECOND);
    }

    #[test]
    fn arithmetic_is_checked_and_never_wraps() {
        assert_eq!(t(5) + d(3), Ok(t(8)));
        assert_eq!(t(5) - d(8), Ok(t(-3)));
        assert_eq!(t(8) - t(5), Ok(d(3)));
        assert!(matches!(t(5) - t(8), Err(TimeError::Negative { .. })));
        assert!((Time::MAX + d(1)).is_err());
        assert!((Time::MIN - d(1)).is_err());
        assert!((Time::MAX - Time::MIN).is_err());
        assert!((Duration::MAX + d(1)).is_err());
        assert!((d(1) - d(2)).is_err());
        assert_eq!(Time::MAX.saturating_add(Duration::MAX), Time::MAX);
        assert_eq!(Time::MIN.saturating_sub(Duration::MAX), Time::MIN);
        assert_eq!(Duration::MAX.saturating_add(Duration::MAX), Duration::MAX);
        assert_eq!(d(1).saturating_sub(d(5)), Duration::ZERO);
        assert_eq!(d(7).checked_mul(3), Ok(d(21)));
        assert!(d(7).checked_mul(-1).is_err());
        assert!(Duration::MAX.checked_mul(2).is_err());
        assert!(d(1).checked_mul(i64::MAX).is_err());
        assert_eq!(t(-3).abs_diff(t(4)), Ok(d(7)));
        assert!(Time::MIN.abs_diff(Time::MAX).is_err());
        assert_eq!(Time::from(d(9)), t(9));
        assert!(Duration::try_from(t(-1)).is_err());
    }

    #[test]
    fn display_is_lossless_and_parses_back() {
        let cases = [
            (0, "0s"),
            (FLICKS_PER_SECOND, "1s"),
            (62 * FLICKS_PER_SECOND + FLICKS_PER_SECOND / 2, "62.5s"),
            (-FLICKS_PER_SECOND / 4, "-0.25s"),
            (29_400_000, "29400000"), // 1/24 s: no terminating decimal
            (1, "1"),
            (-1, "-1"),
            (441, "0.000000625s"),
            (MAX_SAFE_FLICKS, "9007199254740991"),
        ];
        for (flicks, text) in cases {
            assert_eq!(t(flicks).to_string(), text);
            assert_eq!(text.parse::<Time>(), Ok(t(flicks)), "{text}");
        }
        assert_eq!(format!("{:.3}", t(29_400_000)), "0.042s");
        assert_eq!(d(FLICKS_PER_SECOND * 3).to_string(), "3s");
    }

    #[test]
    fn parsing_rejects_bad_input() {
        for bad in [
            "", "s", "1.s", ".5s", "1.5", "+1s", "1e3", "abc", "1 s", "--1s",
        ] {
            assert!(bad.parse::<Time>().is_err(), "{bad:?} should fail");
        }
        assert!(matches!(
            "f:10".parse::<Time>(),
            Err(TimeError::NeedsFrameRate(_))
        ));
        assert!(matches!(
            "00:00:01:00".parse::<Time>(),
            Err(TimeError::NeedsFrameRate(_))
        ));
        assert!(matches!(
            "99999999999999999999s".parse::<Time>(),
            Err(TimeError::OutOfRange { .. })
        ));
        assert!(matches!(
            "9007199254740992".parse::<Time>(),
            Err(TimeError::OutOfRange { .. })
        ));
        assert!("1.0000000000000000001s".parse::<Time>().is_err()); // > 18 decimals
        assert!("-1s".parse::<Duration>().is_err());
    }

    #[test]
    fn seconds_round_to_the_nearest_flick() {
        // 0.333333333 s = 235,199,999.7648 flicks; 0.3333333 s = 235,199,976.48 flicks.
        assert_eq!("0.333333333s".parse::<Time>(), Ok(t(235_200_000)));
        assert_eq!("0.3333333s".parse::<Time>(), Ok(t(235_199_976)));
        assert_eq!("-0.3333333s".parse::<Time>(), Ok(t(-235_199_976)));
        // 0.0000000625 s is 44.1 flicks; 0.0000003125 s is 220.5 (ties away from zero).
        assert_eq!("0.0000000625s".parse::<Time>(), Ok(t(44)));
        assert_eq!("0.0000003125s".parse::<Time>(), Ok(t(221)));
        assert_eq!("-0.0000003125s".parse::<Time>(), Ok(t(-221)));
    }

    #[test]
    fn wire_format_is_a_safe_json_integer() {
        assert_eq!(serde_json::to_string(&t(42)).unwrap(), "42");
        assert_eq!(serde_json::to_string(&d(7)).unwrap(), "7");
        let max = serde_json::json!(MAX_SAFE_FLICKS);
        assert_eq!(serde_json::from_value::<Time>(max).unwrap(), Time::MAX);
        let min = serde_json::json!(-MAX_SAFE_FLICKS);
        assert_eq!(serde_json::from_value::<Time>(min).unwrap(), Time::MIN);
        for bad in [
            serde_json::json!(MAX_SAFE_FLICKS + 1),
            serde_json::json!(-MAX_SAFE_FLICKS - 1),
            serde_json::json!(i64::MAX),
            serde_json::json!(u64::MAX),
            serde_json::json!(2.5),
            serde_json::json!(1e300),
            serde_json::json!("f:3"),
            serde_json::json!(null),
            serde_json::json!(true),
        ] {
            assert!(
                serde_json::from_value::<Time>(bad.clone()).is_err(),
                "{bad}"
            );
        }
        assert_eq!(
            serde_json::from_value::<Time>(serde_json::json!(3.0)).unwrap(),
            t(3)
        );
        assert_eq!(serde_json::from_str::<Time>("1e3").unwrap(), t(1000));
        assert_eq!(
            serde_json::from_value::<Time>(serde_json::json!("2s")).unwrap(),
            Time::from_secs(2).unwrap()
        );
        assert!(serde_json::from_value::<Duration>(serde_json::json!(-1)).is_err());
        assert!(serde_json::from_value::<Duration>(serde_json::json!("-1s")).is_err());
    }

    #[test]
    fn json_schema_depends_on_direction() {
        let input = schemars::generate::SchemaSettings::default()
            .for_deserialize()
            .into_generator()
            .into_root_schema_for::<Time>();
        let output = schemars::generate::SchemaSettings::default()
            .for_serialize()
            .into_generator()
            .into_root_schema_for::<Time>();
        assert!(input.get("anyOf").is_some());
        assert_eq!(output.get("type"), Some(&serde_json::json!("integer")));
        assert_eq!(
            output.get("maximum"),
            Some(&serde_json::json!(MAX_SAFE_FLICKS))
        );
    }
}
