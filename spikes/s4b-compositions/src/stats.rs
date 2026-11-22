// SPDX-License-Identifier: GPL-3.0-or-later
//! Timing summaries and stable hashes for the benchmarks and determinism checks.

use serde::Serialize;

/// Percentile summary of samples (any unit).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Summary {
    pub n: usize,
    pub min: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
}

impl Summary {
    /// Nearest-rank percentiles.
    #[must_use]
    pub fn of(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut v = samples.to_vec();
        v.sort_by(f64::total_cmp);
        let n = v.len();
        let pct = |p: f64| {
            let rank = ((p / 100.0) * n as f64).ceil() as usize;
            v[rank.clamp(1, n) - 1]
        };
        Self {
            n,
            min: v[0],
            p50: pct(50.0),
            p95: pct(95.0),
            p99: pct(99.0),
            max: v[n - 1],
            mean: v.iter().sum::<f64>() / n as f64,
        }
    }
}

/// 64-bit FNV-1a (stable across runs and platforms).
#[must_use]
pub fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = Summary::of(&v);
        assert_eq!((s.p50, s.p95, s.p99, s.max), (50.0, 95.0, 99.0, 100.0));
        assert_eq!(Summary::of(&[]).n, 0);
    }

    #[test]
    fn fnv_known_value() {
        assert_eq!(fnv64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
