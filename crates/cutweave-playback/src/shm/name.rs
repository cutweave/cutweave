// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-launch ring names: `cwp-<pid: 8 hex>-<64 random bits: 16 hex>`.
//!
//! The pid lets the next engine start find rings left behind by a dead engine (stale-ring
//! reaping); the 64 random bits from the OS CSPRNG make the name unguessable. 29 bytes, so the
//! POSIX form `/cwp-…` stays within macOS's 31-byte `PSHMNAMLEN`.

use std::fmt;

use crate::shm::os::ShmError;

/// Prefix of every ring name.
pub const RING_NAME_PREFIX: &str = "cwp-";
/// Length of every ring name in bytes.
pub const RING_NAME_LEN: usize = RING_NAME_PREFIX.len() + 8 + 1 + 16;

/// A validated ring name (without the OS namespace prefix `Local\` or `/`).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RingName(String);

impl RingName {
    /// A fresh name for a ring owned by this process.
    pub fn generate() -> Result<Self, ShmError> {
        let random = random_u64()?;
        Ok(Self(format!(
            "{RING_NAME_PREFIX}{:08x}-{random:016x}",
            std::process::id()
        )))
    }

    /// Validate a name received from elsewhere (`--ring` argument, run-file, successor field).
    pub fn parse(name: &str) -> Result<Self, ShmError> {
        let ok = name.len() == RING_NAME_LEN
            && name.starts_with(RING_NAME_PREFIX)
            && name.as_bytes()[RING_NAME_PREFIX.len() + 8] == b'-'
            && name
                .bytes()
                .enumerate()
                .skip(RING_NAME_PREFIX.len())
                .filter(|&(i, _)| i != RING_NAME_PREFIX.len() + 8)
                .all(|(_, b)| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if ok {
            Ok(Self(name.to_owned()))
        } else {
            Err(ShmError::InvalidName(name.to_owned()))
        }
    }

    /// The name as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Pid of the process that generated the name.
    #[must_use]
    pub fn pid(&self) -> u32 {
        let hex = &self.0[RING_NAME_PREFIX.len()..RING_NAME_PREFIX.len() + 8];
        // `parse`/`generate` guarantee 8 lowercase hex digits.
        u32::from_str_radix(hex, 16).unwrap_or(0)
    }
}

impl fmt::Display for RingName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RingName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RingName({})", self.0)
    }
}

/// 64 bits from the OS CSPRNG.
pub(crate) fn random_u64() -> Result<u64, ShmError> {
    getrandom::u64().map_err(|e| ShmError::Entropy(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_names_are_valid_unique_and_short() {
        let a = RingName::generate().unwrap();
        let b = RingName::generate().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.as_str().len(), RING_NAME_LEN);
        // Fits macOS PSHMNAMLEN (31) with the leading '/'.
        const {
            assert!(RING_NAME_LEN < 31);
        }
        assert_eq!(RingName::parse(a.as_str()).unwrap(), a);
        assert_eq!(a.pid(), std::process::id());
    }

    #[test]
    fn foreign_names_are_rejected() {
        for bad in [
            "",
            "cwp-",
            "cwp-0000abcd-0123456789abcdeF",
            "cwp-0000abcd_0123456789abcdef",
            "cws-0000abcd-0123456789abcdef",
            "cwp-0000abcd-0123456789abcdef0",
            "cwp-0000abc/-0123456789abcdef",
            "../../etc/passwd",
            "Global\\cwp-0000abcd-0123456789",
        ] {
            assert!(
                matches!(RingName::parse(bad), Err(ShmError::InvalidName(_))),
                "{bad:?}"
            );
        }
        let ok = RingName::parse("cwp-0000abcd-0123456789abcdef").unwrap();
        assert_eq!(ok.pid(), 0xabcd);
    }
}
