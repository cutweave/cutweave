// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable 64-bit FNV-1a hashing for render-graph cache keys.
//!
//! `std::hash::DefaultHasher` is not guaranteed stable across Rust releases or processes, and
//! cache keys must be (arch §8: frames cached by `(subgraph hash, t)`, parallel chunked export),
//! so keys hash a canonical little-endian byte encoding.

use std::fmt;

/// Content-addressed key of a pass output: a hash of the pass parameters, its size and its
/// inputs' keys (a Merkle hash). The output pass key identifies the whole frame.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CacheKey(pub u64);

impl fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl fmt::Display for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// FNV-1a 64-bit hasher with typed helpers.
#[derive(Clone, Debug)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    /// A fresh hasher.
    #[must_use]
    pub fn new() -> Self {
        Self(Self::OFFSET)
    }

    /// Hashes raw bytes.
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
        self
    }

    /// Hashes a `u64`.
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    /// Hashes a `u32`.
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    /// Hashes an `f64` by its bits (`-0.0` and `0.0` differ; NaN never reaches a key).
    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.u64(v.to_bits())
    }

    /// Hashes a slice of `f64`.
    pub fn f64s(&mut self, v: &[f64]) -> &mut Self {
        for &x in v {
            self.f64(x);
        }
        self
    }

    /// Hashes a length-prefixed string.
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.u64(s.len() as u64).bytes(s.as_bytes())
    }

    /// The hash value.
    #[must_use]
    pub fn finish(&self) -> u64 {
        self.0
    }

    /// The hash as a cache key.
    #[must_use]
    pub fn key(&self) -> CacheKey {
        CacheKey(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_known_vectors() {
        assert_eq!(Fnv::new().finish(), 0xcbf2_9ce4_8422_2325);
        assert_eq!(Fnv::new().bytes(b"a").finish(), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(Fnv::new().bytes(b"foobar").finish(), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn strings_are_length_prefixed() {
        assert_ne!(
            Fnv::new().str("ab").str("c").finish(),
            Fnv::new().str("a").str("bc").finish()
        );
    }
}
