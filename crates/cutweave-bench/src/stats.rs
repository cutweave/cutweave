// SPDX-License-Identifier: GPL-3.0-or-later
//! Percentiles and the latency histogram of the command round-trip probe.

use serde::{Deserialize, Serialize};

/// Upper bounds of the histogram buckets in milliseconds; one more bucket holds the rest.
pub const BUCKETS_MS: [f64; 12] = [
    0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0,
];

/// The value at fraction `p` (`0.0..=1.0`) of `sorted` (ascending); `0.0` when empty.
#[must_use]
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let i = ((sorted.len() as f64 - 1.0) * p.clamp(0.0, 1.0)).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// What a set of latency samples looks like.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// How many samples.
    pub samples: usize,
    /// Median, ms.
    pub p50: f64,
    /// 95th percentile, ms.
    pub p95: f64,
    /// 99th percentile, ms.
    pub p99: f64,
    /// Slowest, ms.
    pub max: f64,
    /// Samples per bucket of [`BUCKETS_MS`], then the ones above the last bound.
    pub histogram: Vec<u32>,
}

/// Summarizes `ms` (milliseconds, any order).
#[must_use]
pub fn summarize(ms: &[f64]) -> Summary {
    let mut sorted = ms.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mut histogram = vec![0; BUCKETS_MS.len() + 1];
    for v in &sorted {
        let bucket = BUCKETS_MS
            .iter()
            .position(|le| v <= le)
            .unwrap_or(BUCKETS_MS.len());
        histogram[bucket] += 1;
    }
    Summary {
        samples: sorted.len(),
        p50: percentile(&sorted, 0.5),
        p95: percentile(&sorted, 0.95),
        p99: percentile(&sorted, 0.99),
        max: sorted.last().copied().unwrap_or(0.0),
        histogram,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_and_buckets() {
        let ms: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = summarize(&ms);
        assert_eq!(s.samples, 100);
        assert!((s.p50 - 51.0).abs() < 1.0);
        assert!((s.p95 - 95.0).abs() < 1.0);
        assert!((s.max - 100.0).abs() < f64::EPSILON);
        assert_eq!(s.histogram.iter().sum::<u32>(), 100);
        // 1 ms and below, then (1, 2], (2, 4], ...
        assert_eq!(s.histogram[4], 1);
        assert_eq!(s.histogram[5], 1);
        assert_eq!(s.histogram[6], 2);
        assert_eq!(percentile(&[], 0.5), 0.0);
    }
}
