// SPDX-License-Identifier: GPL-3.0-or-later
//! Small lock-free helpers (atomic floats), level conversions and CLI duration parsing.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// `f32` stored in an `AtomicU32` (bit pattern); used for parameters shared with the RT thread.
#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    /// New atomic holding `v`.
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    /// Relaxed load.
    #[inline]
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    /// Relaxed store.
    #[inline]
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// `f64` stored in an `AtomicU64` (bit pattern).
#[derive(Debug, Default)]
pub struct AtomicF64(AtomicU64);

impl AtomicF64 {
    /// New atomic holding `v`.
    pub fn new(v: f64) -> Self {
        Self(AtomicU64::new(v.to_bits()))
    }
    /// Relaxed load.
    #[inline]
    pub fn load(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    /// Relaxed store.
    #[inline]
    pub fn store(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// Decibels to linear amplitude.
#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Linear amplitude to decibels (`-inf` for 0).
#[inline]
pub fn lin_to_db(lin: f32) -> f32 {
    20.0 * lin.log10()
}

/// Parses `30m`, `10s`, `1h`, `250ms`, `1.5s` or a bare number of seconds.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num, mult) = if let Some(v) = s.strip_suffix("ms") {
        (v, 0.001)
    } else if let Some(v) = s.strip_suffix('s') {
        (v, 1.0)
    } else if let Some(v) = s.strip_suffix('m') {
        (v, 60.0)
    } else if let Some(v) = s.strip_suffix('h') {
        (v, 3600.0)
    } else {
        (s, 1.0)
    };
    let value: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid duration `{s}` (use e.g. 30m, 10s, 250ms)"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("invalid duration `{s}`"));
    }
    Ok(Duration::from_secs_f64(value * mult))
}

/// Nanoseconds in a second.
pub const NANOS: f64 = 1_000_000_000.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("1.5").unwrap(), Duration::from_millis(1500));
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("-3s").is_err());
    }

    #[test]
    fn db_roundtrip() {
        assert!((db_to_lin(-6.0) - 0.501_187).abs() < 1e-5);
        assert!((lin_to_db(db_to_lin(-23.0)) + 23.0).abs() < 1e-4);
        let a = AtomicF32::new(1.5);
        a.store(-2.25);
        assert_eq!(a.load(), -2.25);
    }
}
