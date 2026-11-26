// SPDX-License-Identifier: GPL-3.0-or-later
//! Constant playback speed, reverse and freeze frames (WP-2D.01; arch §4.2; timeline spec INV-47),
//! and the one mapping from a clip's time to its media: [`source_time`].
//!
//! # Speed
//!
//! [`Speed`] is an exact ratio `num/den` from 1/10 to 100, in lowest terms with both terms at
//! most [`MAX_SPEED_TERM`]; never a float, so frame math stays exact (2× over 10 s is 5 s to the
//! flick, 1/3× is exact too). A timeline span `d` plays `round(d × speed)` flicks of media,
//! rounded half away from zero ([`Speed::source`]): the rounding is odd-symmetric, so moving an
//! edge by `δ` and back by `−δ` restores a source position exactly (INV-47).
//!
//! # The mapping
//!
//! A clip of length `len` at speed `s` plays the media range `[source_in, source_out)` with
//! `source_out = source_in + round(len × s)` ([`SourceMap`]). At item time `o` (`0 ≤ o < len`) it
//! shows:
//!
//! - forwards: `source_in + round(o × s)`;
//! - reversed: `source_out − 1 flick − round(o × s)`, so the first frame shown is the last frame of
//!   the range, not the first frame after it;
//! - a freeze frame: `freeze.at_source` at every `o`.
//!
//! Results never leave the played range. Render, audio, thumbnails and export all call
//! [`source_time`] / [`SourceMap::at`]; the picture is the frame on screen at that media time
//! (frame-nearest, no blending between frames until P4).
//!
//! # Parts of a clip (splits, cuts, trims)
//!
//! A forward clip is anchored at `source_in` and a reversed one at `source_out`: the part of a
//! clip that starts `o` into it plays from `source_in + round(o × s)` forwards, or ends at
//! `source_out − round(o × s)` reversed ([`SourceMap::part`]). A part that rounding would put one
//! flick outside the media is nudged back inside it.

use std::{borrow::Cow, fmt};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::{Clip, Time};

/// The largest numerator or denominator of a [`Speed`] (in lowest terms).
pub const MAX_SPEED_TERM: u32 = 100_000;

/// A speed outside 1/10..=100, with a zero or too large term, or text that is not a speed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SpeedError(String);

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// `n / d` rounded half away from zero (`d > 0`).
fn div_round(n: i128, d: i128) -> i128 {
    let (q, r) = (n / d, n % d);
    if 2 * r.abs() >= d { q + n.signum() } else { q }
}

fn saturate(v: i128) -> i64 {
    i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
}

/// A constant playback speed: `num/den` times normal, from 1/10 to 100, in lowest terms.
///
/// Serialized as `{"num": 2, "den": 1}`; unreduced input is reduced on parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(try_from = "RawSpeed")]
// The validating `try_from` does not change the shape; specta only needs the fields.
#[specta(skip_attr = "serde")]
pub struct Speed {
    num: u32,
    den: u32,
}

/// The unvalidated wire form of a [`Speed`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpeed {
    num: u32,
    den: u32,
}

impl TryFrom<RawSpeed> for Speed {
    type Error = SpeedError;
    fn try_from(raw: RawSpeed) -> Result<Speed, SpeedError> {
        Speed::new(raw.num, raw.den)
    }
}

impl Default for Speed {
    fn default() -> Speed {
        Speed::ONE
    }
}

impl Speed {
    /// Normal speed.
    pub const ONE: Speed = Speed { num: 1, den: 1 };
    /// The slowest speed, 1/10.
    pub const MIN: Speed = Speed { num: 1, den: 10 };
    /// The fastest speed, 100.
    pub const MAX: Speed = Speed { num: 100, den: 1 };

    /// `num/den`, reduced to lowest terms. `Err` if a term is zero, the speed is outside
    /// 1/10..=100, or a reduced term exceeds [`MAX_SPEED_TERM`].
    pub fn new(num: u32, den: u32) -> Result<Speed, SpeedError> {
        if num == 0 || den == 0 {
            return Err(SpeedError(format!(
                "a speed needs a positive numerator and denominator, not {num}/{den}"
            )));
        }
        Speed::reduce(u64::from(num), u64::from(den))
    }

    /// `num/den` (any size) reduced, then checked like [`Speed::new`].
    fn reduce(num: u64, den: u64) -> Result<Speed, SpeedError> {
        let g = gcd(num, den).max(1);
        let (n, d) = (num / g, den / g);
        // 1/10 <= n/d <= 100
        if n * 10 < d || n > d * 100 {
            return Err(SpeedError(format!(
                "speed {n}/{d} is out of range: use 0.1x to 100x"
            )));
        }
        match (u32::try_from(n), u32::try_from(d)) {
            (Ok(num), Ok(den)) if num <= MAX_SPEED_TERM && den <= MAX_SPEED_TERM => {
                Ok(Speed { num, den })
            }
            _ => Err(SpeedError(format!(
                "speed {n}/{d} is too precise: use a fraction with terms up to {MAX_SPEED_TERM} \
                 (for example \"4/3\") or at most 5 decimals"
            ))),
        }
    }

    /// Parses `"2"`, `"0.5"`, `"1/3"`, `"2x"` or `"50%"` exactly (decimals become fractions).
    pub fn parse(text: &str) -> Result<Speed, SpeedError> {
        let bad = || {
            SpeedError(format!(
                "`{text}` is not a speed: use a number (\"2\", \"0.5\"), a fraction (\"1/3\"), \
                 \"2x\" or \"50%\""
            ))
        };
        let s = text.trim();
        let (s, percent) = match s.strip_suffix('%') {
            Some(rest) => (rest.trim_end(), true),
            None => (
                s.strip_suffix(['x', 'X', '×']).map_or(s, str::trim_end),
                false,
            ),
        };
        let (num, den) = if let Some((n, d)) = s.split_once('/') {
            let n: u64 = n.trim().parse().map_err(|_| bad())?;
            let d: u64 = d.trim().parse().map_err(|_| bad())?;
            (n, d)
        } else {
            decimal(s).ok_or_else(bad)?
        };
        let den = if percent {
            den.checked_mul(100)
        } else {
            Some(den)
        }
        .ok_or_else(bad)?;
        if num == 0 || den == 0 {
            return Err(bad());
        }
        Speed::reduce(num, den)
    }

    /// The exact value of a JSON number (its shortest decimal form: `0.1` is 1/10).
    pub fn from_f64(v: f64) -> Result<Speed, SpeedError> {
        if !v.is_finite() || v <= 0.0 {
            return Err(SpeedError(format!(
                "speed {v} is out of range: use 0.1x to 100x"
            )));
        }
        let text = format!("{v}");
        let (num, den) = decimal(&text).ok_or_else(|| {
            SpeedError(format!(
                "speed {v} is too precise: use a fraction like \"4/3\""
            ))
        })?;
        Speed::reduce(num, den)
    }

    /// The numerator.
    #[must_use]
    pub const fn num(self) -> u32 {
        self.num
    }

    /// The denominator.
    #[must_use]
    pub const fn den(self) -> u32 {
        self.den
    }

    /// Whether this is normal speed.
    #[must_use]
    pub const fn is_one(self) -> bool {
        self.num == 1 && self.den == 1
    }

    /// The speed as `f64`, for display and audio rates only (never for time math).
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    /// The media span a timeline span `d` plays: `d × speed` rounded half away from zero, so
    /// `source(-d) == -source(d)` (INV-47).
    #[must_use]
    pub fn source(self, d: i64) -> i64 {
        saturate(div_round(
            i128::from(d) * i128::from(self.num),
            i128::from(self.den),
        ))
    }

    /// The longest timeline span (at least 0) whose media span fits in `room` flicks:
    /// `source(len) <= room`.
    #[must_use]
    pub fn longest_within(self, room: i64) -> i64 {
        // source(len) <= room  <=>  len × num < (room + 1/2) × den  (half away from zero)
        //                     <=>  2·len·num < (2·room + 1)·den
        let limit = (2 * i128::from(room) + 1) * i128::from(self.den);
        let step = 2 * i128::from(self.num);
        saturate(((limit + step - 1).div_euclid(step) - 1).max(0))
    }

    /// `d × self / other` as an exact ratio `(num, den)`: the timeline length that plays at
    /// `other` what `d` plays at `self` is `d × num / den`.
    #[must_use]
    pub fn ratio_to(self, other: Speed) -> (u64, u64) {
        let num = u64::from(self.num) * u64::from(other.den);
        let den = u64::from(self.den) * u64::from(other.num);
        let g = gcd(num, den).max(1);
        (num / g, den / g)
    }
}

/// The digits of a plain decimal (`"12"`, `"0.125"`, `"1e-1"` is refused) as `(num, 10^k)`.
fn decimal(s: &str) -> Option<(u64, u64)> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let frac = frac.trim_end_matches('0');
    // More than 12 decimals cannot reduce to terms of at most 100,000 anyway.
    if frac.len() > 12 || int.len() > 6 {
        return None;
    }
    let den = 10_u64.pow(u32::try_from(frac.len()).ok()?);
    let int: u64 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let frac_v: u64 = if frac.is_empty() {
        0
    } else {
        frac.parse().ok()?
    };
    Some((int.checked_mul(den)?.checked_add(frac_v)?, den))
}

/// `2`, `0.5`, `1.25`, or `1/3` when the value has no short decimal form.
impl fmt::Display for Speed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            return write!(f, "{}", self.num);
        }
        // Exact decimals: the denominator has only the prime factors 2 and 5.
        let mut d = self.den;
        let (mut twos, mut fives) = (0_u32, 0_u32);
        while d.is_multiple_of(2) {
            d /= 2;
            twos += 1;
        }
        while d.is_multiple_of(5) {
            d /= 5;
            fives += 1;
        }
        let places = twos.max(fives);
        if d == 1 && places <= 6 {
            let scale = 10_u64.pow(places);
            let scaled = u64::from(self.num) * (scale / u64::from(self.den));
            let int = scaled / scale;
            let frac = scaled % scale;
            let digits = format!("{frac:0width$}", width = places as usize);
            write!(f, "{int}.{}", digits.trim_end_matches('0'))
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl JsonSchema for Speed {
    fn schema_name() -> Cow<'static, str> {
        "Speed".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A playback speed as an exact fraction of normal speed, from 1/10 to 100: {\"num\": 2, \"den\": 1} plays twice as fast, {\"num\": 1, \"den\": 3} at a third.",
            "type": "object",
            "properties": {
                "num": { "type": "integer", "minimum": 1, "maximum": MAX_SPEED_TERM, "description": "Numerator." },
                "den": { "type": "integer", "minimum": 1, "maximum": MAX_SPEED_TERM, "description": "Denominator." }
            },
            "required": ["num", "den"],
            "additionalProperties": false
        })
    }
}

/// A freeze frame (WP-2D.01): the clip shows one frame of its media for its whole length, and is
/// silent.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(deny_unknown_fields)]
pub struct Freeze {
    /// The media position of the frame shown.
    pub at_source: Time,
}

/// How a clip maps its own time to its media (the module docs): the canonical source-time
/// mapping and the source position of a part of the clip. All values are flicks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceMap {
    /// Media position of the start of the played range.
    pub source_in: i64,
    /// Timeline length of the clip.
    pub len: i64,
    /// Playback speed.
    pub speed: Speed,
    /// Played backwards.
    pub reverse: bool,
    /// The frozen media position of a freeze frame.
    pub freeze: Option<i64>,
}

impl SourceMap {
    /// The map of `clip`.
    #[must_use]
    pub fn of(clip: &Clip) -> SourceMap {
        SourceMap {
            source_in: clip.source_in.flicks(),
            len: clip.duration.flicks(),
            speed: clip.speed,
            reverse: clip.reverse,
            freeze: clip.freeze.map(|f| f.at_source.flicks()),
        }
    }

    /// The length of media played: `round(len × speed)` (a freeze frame's speed is ignored by
    /// the mapping).
    #[must_use]
    pub fn span(&self) -> i64 {
        self.speed.source(self.len)
    }

    /// The end of the played range (exclusive): `source_in + span`.
    #[must_use]
    pub fn source_out(&self) -> i64 {
        self.source_in.saturating_add(self.span())
    }

    /// The media position shown and heard at item time `offset` (`0 ≤ offset < len`; `None`
    /// outside): the canonical mapping of the module docs.
    #[must_use]
    pub fn at(&self, offset: i64) -> Option<i64> {
        if offset < 0 || offset >= self.len {
            return None;
        }
        if let Some(frozen) = self.freeze {
            return Some(frozen);
        }
        let last = (self.span() - 1).max(0);
        let moved = self.speed.source(offset).min(last);
        Some(if self.reverse {
            self.source_in.saturating_add(last - moved)
        } else {
            self.source_in.saturating_add(moved)
        })
    }

    /// The `source_in` of the part of the clip that starts `offset` into it and lasts `len`
    /// (splits, cuts and trims): forward clips keep their anchor at `source_in`, reversed ones
    /// at `source_out`; a freeze frame keeps its position. With the media's length (`media_len`,
    /// bounded media only) a result that rounding put one flick outside the media is nudged back.
    #[must_use]
    pub fn part(&self, offset: i64, len: i64, media_len: Option<i64>) -> i64 {
        if self.freeze.is_some() {
            return self.source_in;
        }
        let span = self.speed.source(len);
        let start = if self.reverse {
            self.source_out()
                .saturating_sub(self.speed.source(offset))
                .saturating_sub(span)
        } else {
            self.source_in.saturating_add(self.speed.source(offset))
        };
        fit(start, span, media_len)
    }
}

/// `source_in` moved (by rounding, at most a flick) so that `[source_in, source_in + span)` lies
/// inside `[0, media_len]`.
#[must_use]
pub fn fit(source_in: i64, span: i64, media_len: Option<i64>) -> i64 {
    let mut s = source_in;
    if let Some(total) = media_len
        && s.saturating_add(span) > total
        && s.saturating_add(span) - total <= 1
    {
        s = total.saturating_sub(span);
    }
    if s == -1 { 0 } else { s }
}

/// `source_time(item, t)` (WP-2D.01): the media time `clip` shows and plays at sequence time
/// `t`, or `None` outside the clip. The single mapping render, audio, thumbnails and export use.
#[must_use]
pub fn source_time(clip: &Clip, t: Time) -> Option<Time> {
    let offset = t.flicks().checked_sub(clip.start.flicks())?;
    let flicks = SourceMap::of(clip).at(offset)?;
    Time::from_flicks(flicks).ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn sp(n: u32, d: u32) -> Speed {
        Speed::new(n, d).unwrap()
    }

    fn map(source_in: i64, len: i64, speed: Speed, reverse: bool) -> SourceMap {
        SourceMap {
            source_in,
            len,
            speed,
            reverse,
            freeze: None,
        }
    }

    #[test]
    fn speeds_are_reduced_and_bounded() {
        assert_eq!(sp(4, 2), sp(2, 1));
        assert_eq!((sp(6, 18).num(), sp(6, 18).den()), (1, 3));
        assert!(Speed::new(1, 11).is_err());
        assert!(Speed::new(101, 1).is_err());
        assert!(Speed::new(0, 1).is_err());
        assert!(Speed::new(1, 0).is_err());
        assert!(Speed::new(99_991, 100_000).is_ok());
        assert!(Speed::reduce(100_001, 100_002).is_err());
        assert_eq!(Speed::MIN, sp(1, 10));
        assert_eq!(Speed::MAX, sp(100, 1));
    }

    #[test]
    fn speeds_parse_exactly() {
        assert_eq!(Speed::parse("2").unwrap(), sp(2, 1));
        assert_eq!(Speed::parse("0.5").unwrap(), sp(1, 2));
        assert_eq!(Speed::parse(" 1/3 ").unwrap(), sp(1, 3));
        assert_eq!(Speed::parse("2x").unwrap(), sp(2, 1));
        assert_eq!(Speed::parse("1.25×").unwrap(), sp(5, 4));
        assert_eq!(Speed::parse("50%").unwrap(), sp(1, 2));
        assert_eq!(Speed::parse(".1").unwrap(), sp(1, 10));
        for bad in [
            "", "x", "-1", "abc", "1/0", "0", "1e1", "0.05", "200", "1.2.3",
        ] {
            assert!(Speed::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(Speed::from_f64(0.1).unwrap(), sp(1, 10));
        assert_eq!(Speed::from_f64(2.0).unwrap(), sp(2, 1));
        assert_eq!(Speed::from_f64(1.5).unwrap(), sp(3, 2));
        assert!(Speed::from_f64(1.0 / 3.0).is_err());
        assert!(Speed::from_f64(f64::NAN).is_err());
    }

    #[test]
    fn speeds_display_short() {
        assert_eq!(sp(2, 1).to_string(), "2");
        assert_eq!(sp(1, 2).to_string(), "0.5");
        assert_eq!(sp(5, 4).to_string(), "1.25");
        assert_eq!(sp(1, 3).to_string(), "1/3");
        assert_eq!(sp(1, 10).to_string(), "0.1");
        for s in [sp(1, 2), sp(5, 4), sp(1, 3), sp(7, 8), sp(100, 1)] {
            assert_eq!(Speed::parse(&s.to_string()).unwrap(), s);
        }
    }

    #[test]
    fn speeds_serialize_as_fractions() {
        let s: Speed = serde_json::from_str(r#"{"num": 4, "den": 8}"#).unwrap();
        assert_eq!(s, sp(1, 2));
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"num":1,"den":2}"#);
        assert!(serde_json::from_str::<Speed>(r#"{"num": 1, "den": 20}"#).is_err());
        assert!(serde_json::from_str::<Speed>(r#"{"num": 1, "den": 2, "x": 1}"#).is_err());
    }

    #[test]
    fn source_rounds_half_away_from_zero_and_is_odd() {
        let s = sp(3, 2);
        assert_eq!(s.source(1), 2);
        assert_eq!(s.source(-1), -2);
        assert_eq!(s.source(2), 3);
        assert_eq!(Speed::ONE.source(-7), -7);
        assert_eq!(sp(1, 3).source(1), 0);
        for n in [1_u32, 2, 3, 7, 13] {
            for d in [1_u32, 2, 3, 7, 10] {
                let Ok(s) = Speed::new(n, d) else { continue };
                for room in -3..200 {
                    let len = s.longest_within(room);
                    assert!(len == 0 || s.source(len) <= room, "{n}/{d} {room}");
                    assert!(s.source(len + 1) > room, "{n}/{d} {room}");
                }
            }
        }
    }

    #[test]
    fn exact_ratios_are_exact_in_flicks() {
        const SEC: i64 = 705_600_000;
        // 2× over 10 s plays 20 s of media; a third plays 3 1/3 s.
        assert_eq!(sp(2, 1).source(10 * SEC), 20 * SEC);
        assert_eq!(sp(1, 3).source(30 * SEC), 10 * SEC);
        assert_eq!(sp(1, 2).ratio_to(sp(2, 1)), (1, 4));
        assert_eq!(Speed::ONE.ratio_to(sp(1, 3)), (3, 1));
    }

    #[test]
    fn forward_mapping_moves_with_speed_and_stays_inside() {
        let m = map(100, 50, sp(2, 1), false);
        assert_eq!(m.span(), 100);
        assert_eq!(m.at(0), Some(100));
        assert_eq!(m.at(25), Some(150));
        assert_eq!(m.at(49), Some(198));
        assert_eq!(m.at(50), None);
        assert_eq!(m.at(-1), None);
        let slow = map(0, 3, sp(1, 2), false);
        assert_eq!(
            (slow.at(0), slow.at(1), slow.at(2)),
            (Some(0), Some(1), Some(1))
        );
    }

    #[test]
    fn reverse_starts_at_the_last_flick_of_the_range() {
        let m = map(100, 50, Speed::ONE, true);
        assert_eq!(m.at(0), Some(149));
        assert_eq!(m.at(49), Some(100));
        let fast = map(100, 50, sp(2, 1), true);
        assert_eq!(fast.at(0), Some(199));
        assert_eq!(fast.at(49), Some(101));
    }

    #[test]
    fn a_freeze_shows_one_position() {
        let m = SourceMap {
            freeze: Some(42),
            ..map(100, 50, sp(2, 1), true)
        };
        assert_eq!((m.at(0), m.at(49), m.at(50)), (Some(42), Some(42), None));
        assert_eq!(m.part(10, 20, Some(1000)), 100);
    }

    #[test]
    fn parts_keep_the_anchor() {
        let fwd = map(100, 60, sp(2, 1), false);
        assert_eq!(fwd.part(0, 20, None), 100);
        assert_eq!(fwd.part(20, 40, None), 140);
        let rev = map(100, 60, sp(2, 1), true);
        // Reversed: the left part (timeline first) plays the end of the range.
        assert_eq!(rev.part(0, 20, None), 220 - 40);
        assert_eq!(rev.part(20, 40, None), 100);
        // Splitting a reversed clip keeps every position it shows.
        let left = map(rev.part(0, 20, None), 20, sp(2, 1), true);
        let right = map(rev.part(20, 40, None), 40, sp(2, 1), true);
        for o in 0..60 {
            let got = if o < 20 { left.at(o) } else { right.at(o - 20) };
            assert_eq!(got, rev.at(o), "{o}");
        }
    }

    #[test]
    fn parts_are_nudged_inside_the_media() {
        assert_eq!(fit(11, 90, Some(100)), 10);
        assert_eq!(fit(-1, 90, Some(100)), 0);
        assert_eq!(fit(20, 90, Some(100)), 20, "only rounding is corrected");
        assert_eq!(fit(5, 90, None), 5);
    }
}
