// SPDX-License-Identifier: GPL-3.0-or-later
//! Timeline-to-source time mapping for constant speed and reverse (arch §4.2 `Speed`).
//!
//! Schema v1 clips always play at [`Speed::NORMAL`]; the mapping exists so the evaluator, the
//! scene description and the tests already cover constant speed and reverse, and speed becomes
//! a pure model change later. Speed ramps (`Curve`) are out of scope.

use cutweave_model::{Duration, Time};

/// A constant playback speed, `num / den` times normal, optionally reversed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Speed {
    /// Numerator (positive).
    pub num: u32,
    /// Denominator (positive).
    pub den: u32,
    /// Play the source range backwards.
    pub reverse: bool,
}

impl Speed {
    /// Normal speed, forwards.
    pub const NORMAL: Speed = Speed {
        num: 1,
        den: 1,
        reverse: false,
    };

    /// `Some` when `num` and `den` are both non-zero.
    #[must_use]
    pub const fn new(num: u32, den: u32, reverse: bool) -> Option<Speed> {
        if num == 0 || den == 0 {
            None
        } else {
            Some(Speed { num, den, reverse })
        }
    }

    /// `x * num / den` in wide integers, rounded toward zero. `x` is non-negative.
    fn scale(self, x: i64) -> i128 {
        i128::from(x) * i128::from(self.num) / i128::from(self.den)
    }

    /// The source length played by a clip of `duration`.
    #[must_use]
    pub fn source_span(self, duration: Duration) -> i128 {
        self.scale(duration.flicks())
    }

    /// Maps `offset` (time since the clip start, within `0..duration`) to a source time inside
    /// the half-open played range `[source_in, source_in + span)`.
    ///
    /// Forwards: `source_in + offset * speed`. Reversed: the range is traversed from its end,
    /// `source_in + span - 1 flick - offset * speed`, so the first reversed frame is the last
    /// frame of the range, not the first frame after it. Results never leave the range.
    /// `None` when the clip plays an empty range or the result is not representable.
    #[must_use]
    pub fn map(self, source_in: Time, duration: Duration, offset: Duration) -> Option<Time> {
        let span = self.source_span(duration);
        if span <= 0 {
            return None;
        }
        let last = span - 1;
        let scaled = self.scale(offset.flicks()).min(last);
        let within = if self.reverse { last - scaled } else { scaled };
        let flicks = i64::try_from(i128::from(source_in.flicks()) + within).ok()?;
        Time::from_flicks(flicks).ok()
    }
}

impl Default for Speed {
    fn default() -> Speed {
        Speed::NORMAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(f: i64) -> Time {
        Time::from_flicks(f).unwrap()
    }
    fn d(f: i64) -> Duration {
        Duration::from_flicks(f).unwrap()
    }

    #[test]
    fn normal_speed_is_an_offset() {
        assert_eq!(Speed::NORMAL.map(t(100), d(50), d(0)), Some(t(100)));
        assert_eq!(Speed::NORMAL.map(t(100), d(50), d(49)), Some(t(149)));
    }

    #[test]
    fn double_speed_covers_twice_the_source() {
        let s = Speed::new(2, 1, false).unwrap();
        assert_eq!(s.source_span(d(50)), 100);
        assert_eq!(s.map(t(10), d(50), d(25)), Some(t(60)));
        assert_eq!(s.map(t(10), d(50), d(49)), Some(t(10 + 98)));
    }

    #[test]
    fn half_speed_clamps_inside_the_range() {
        let s = Speed::new(1, 2, false).unwrap();
        // span = 1: every offset maps to the only source flick.
        assert_eq!(s.map(t(0), d(3), d(2)), Some(t(0)));
    }

    #[test]
    fn reverse_starts_at_the_last_source_flick() {
        let s = Speed::new(1, 1, true).unwrap();
        assert_eq!(s.map(t(100), d(50), d(0)), Some(t(149)));
        assert_eq!(s.map(t(100), d(50), d(49)), Some(t(100)));
    }

    #[test]
    fn empty_range_has_no_mapping() {
        assert_eq!(Speed::NORMAL.map(t(0), d(0), d(0)), None);
        assert_eq!(Speed::new(0, 1, false), None);
        assert_eq!(Speed::new(1, 0, false), None);
    }
}
