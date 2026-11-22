// SPDX-License-Identifier: GPL-3.0-or-later
//! Wall-clock time for the document actor: [`WallTime`] and the injectable [`Clock`].
//!
//! The `request_id` dedup window (arch §5.1, ADR-0015) is measured in **wall-clock** time so it
//! survives an engine restart: the journal persists each transaction's timestamp (WP-1C.02).
//! Tests and `--deterministic-ids` runs inject a [`ManualClock`]. Command code never reads the
//! clock (determinism, AGENTS.md); only the actor does, to stamp requests.

use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde::{Deserialize, Serialize};

/// A wall-clock instant: milliseconds since the Unix epoch.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    specta::Type,
)]
#[serde(transparent)]
pub struct WallTime(#[specta(type = specta_typescript::Number)] u64);

impl WallTime {
    /// 1970-01-01T00:00:00Z.
    pub const UNIX_EPOCH: WallTime = WallTime(0);

    /// The instant `ms` milliseconds after the Unix epoch.
    #[must_use]
    pub const fn from_millis(ms: u64) -> WallTime {
        WallTime(ms)
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    /// `self + d` (saturating).
    #[must_use]
    pub fn saturating_add(self, d: Duration) -> WallTime {
        WallTime(self.0.saturating_add(duration_millis(d)))
    }

    /// How long after `earlier` this is; zero if it is not later (the wall clock can step
    /// backwards).
    #[must_use]
    pub const fn saturating_since(self, earlier: WallTime) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }
}

impl fmt::Display for WallTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

fn duration_millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// A source of wall-clock time, injected into the document actor.
pub trait Clock: Send + Sync + fmt::Debug {
    /// The current wall-clock time.
    fn now(&self) -> WallTime;
}

/// The operating system's wall clock.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl Clock for SystemClock {
    fn now(&self) -> WallTime {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        WallTime(duration_millis(since))
    }
}

/// A clock that only moves when told to: tests ("9 minutes later") and deterministic runs.
#[derive(Debug, Default)]
pub struct ManualClock {
    ms: AtomicU64,
}

impl ManualClock {
    /// A clock stopped at `start`.
    #[must_use]
    pub const fn new(start: WallTime) -> ManualClock {
        ManualClock {
            ms: AtomicU64::new(start.0),
        }
    }

    /// Moves the clock forward by `d`.
    pub fn advance(&self, d: Duration) {
        self.ms.fetch_add(duration_millis(d), Ordering::SeqCst);
    }

    /// Sets the clock (also backwards, like a wall clock adjusted by NTP).
    pub fn set(&self, t: WallTime) {
        self.ms.store(t.0, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> WallTime {
        WallTime(self.ms.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_moves_only_when_told() {
        let clock = ManualClock::new(WallTime::from_millis(1_000));
        assert_eq!(clock.now(), WallTime::from_millis(1_000));
        clock.advance(Duration::from_mins(9));
        assert_eq!(clock.now().as_millis(), 1_000 + 540_000);
        clock.set(WallTime::from_millis(10));
        assert_eq!(clock.now(), WallTime::from_millis(10));
    }

    #[test]
    fn wall_time_arithmetic_saturates() {
        let t = WallTime::from_millis(5_000);
        assert_eq!(
            t.saturating_since(WallTime::from_millis(2_000)),
            Duration::from_secs(3)
        );
        assert_eq!(
            t.saturating_since(WallTime::from_millis(9_000)),
            Duration::ZERO
        );
        assert_eq!(
            t.saturating_add(Duration::from_millis(1)),
            WallTime::from_millis(5_001)
        );
        assert_eq!(
            WallTime::from_millis(u64::MAX).saturating_add(Duration::from_secs(1)),
            WallTime::from_millis(u64::MAX)
        );
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    #[test]
    fn system_clock_is_after_2020() {
        assert!(SystemClock.now().as_millis() > 1_577_836_800_000);
    }
}
