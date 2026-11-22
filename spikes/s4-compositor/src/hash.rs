//! Stable 64-bit FNV-1a hashing for render-graph cache keys.
//!
//! `std::hash::DefaultHasher` is not guaranteed stable across Rust releases, and cache keys
//! must be stable across processes (parallel chunked export, §8), so we hash a canonical byte
//! encoding ourselves.

use std::fmt;

/// Content-addressed key of a pass output: hash of the pass parameters and its inputs' keys.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CacheKey(pub u64);

impl fmt::Debug for CacheKey {
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

    #[must_use]
    pub fn new() -> Self {
        Self(Self::OFFSET)
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.u32(v.to_bits())
    }

    pub fn f32s(&mut self, v: &[f32]) -> &mut Self {
        for &x in v {
            self.f32(x);
        }
        self
    }

    pub fn str(&mut self, s: &str) -> &mut Self {
        self.u64(s.len() as u64).bytes(s.as_bytes())
    }

    #[must_use]
    pub fn finish(&self) -> u64 {
        self.0
    }

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
}
