// SPDX-License-Identifier: GPL-3.0-or-later
//! Small helpers kept dependency-free: CRC-32, a seeded PRNG and percentiles.

/// CRC-32 (IEEE 802.3, reflected, init and xor-out `0xFFFF_FFFF`): the same function as zlib and
/// FFmpeg's `framehash -hash crc32`, so per-frame hashes compare directly.
#[derive(Clone)]
pub struct Crc32 {
    table: [u32; 256],
}

impl Default for Crc32 {
    fn default() -> Self {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        Crc32 { table }
    }
}

impl Crc32 {
    /// Continues a running CRC (`state` starts at `0xFFFF_FFFF`).
    pub fn update(&self, mut state: u32, data: &[u8]) -> u32 {
        for &b in data {
            state = self.table[((state ^ u32::from(b)) & 0xFF) as usize] ^ (state >> 8);
        }
        state
    }

    /// One-shot CRC of `data`.
    pub fn checksum(&self, data: &[u8]) -> u32 {
        self.update(0xFFFF_FFFF, data) ^ 0xFFFF_FFFF
    }
}

/// SplitMix64: tiny, seeded, reproducible across platforms (seeks must be replayable).
pub struct Rng(u64);

impl Rng {
    /// A generator with a fixed seed.
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`; the modulo bias is irrelevant at these sizes).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// Nearest-rank percentile of an unsorted sample (`p` in 0..=100). `None` when empty.
pub fn percentile(samples: &[f64], p: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut v = samples.to_vec();
    v.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * v.len() as f64).ceil() as usize;
    Some(v[rank.clamp(1, v.len()) - 1])
}

/// Mean of a sample, 0 when empty.
pub fn mean(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        0.0
    } else {
        samples.iter().sum::<f64>() / samples.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(Crc32::default().checksum(b"123456789"), 0xCBF4_3926);
        assert_eq!(Crc32::default().checksum(b""), 0);
    }

    #[test]
    fn crc32_streaming_equals_one_shot() {
        let c = Crc32::default();
        let s = c.update(0xFFFF_FFFF, b"12345");
        assert_eq!(c.update(s, b"6789") ^ 0xFFFF_FFFF, 0xCBF4_3926);
    }

    #[test]
    fn rng_is_reproducible() {
        let a: Vec<u64> = (0..4)
            .map({
                let mut r = Rng::new(7);
                move |_| r.next_u64()
            })
            .collect();
        let b: Vec<u64> = (0..4)
            .map({
                let mut r = Rng::new(7);
                move |_| r.next_u64()
            })
            .collect();
        assert_eq!(a, b);
        assert_ne!(a[0], a[1]);
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), Some(50.0));
        assert_eq!(percentile(&v, 95.0), Some(95.0));
        assert_eq!(percentile(&v, 100.0), Some(100.0));
        assert_eq!(percentile(&[3.0], 95.0), Some(3.0));
        assert_eq!(percentile(&[], 50.0), None);
    }
}
