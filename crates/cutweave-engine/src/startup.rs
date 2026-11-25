// SPDX-License-Identifier: GPL-3.0-or-later
//! Where an engine start spends its time (issue #143): named phases, logged as one line
//! (`engine start phases`, `subsystem start phases`) with each phase's milliseconds, so a slow
//! start on someone's machine can be read from the log they attach to a bug.

use std::fmt;
use std::time::{Duration, Instant};

/// Milliseconds of `d`, saturating.
#[must_use]
pub(crate) fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Consecutive start phases: each [`mark`](Self::mark) closes the phase that ran since the
/// previous one.
#[derive(Debug)]
pub(crate) struct Phases {
    began: Instant,
    last: Instant,
    marks: Vec<(&'static str, Duration)>,
}

impl Phases {
    /// Starts timing now.
    pub(crate) fn new() -> Phases {
        let now = Instant::now();
        Phases {
            began: now,
            last: now,
            marks: Vec::new(),
        }
    }

    /// Ends the phase `name` (the time since the previous mark).
    pub(crate) fn mark(&mut self, name: &'static str) {
        let now = Instant::now();
        self.marks
            .push((name, now.saturating_duration_since(self.last)));
        self.last = now;
    }

    /// The time since [`new`](Self::new).
    pub(crate) fn total(&self) -> Duration {
        self.began.elapsed()
    }
}

/// `name=ms` pairs in order: `runtime=3 project=12 subsystems=410 server=6`.
impl fmt::Display for Phases {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (name, took)) in self.marks.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{name}={}", ms(*took))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_print_in_order_with_their_milliseconds() {
        let mut p = Phases::new();
        p.mark("a");
        p.mark("b");
        let text = p.to_string();
        assert!(text.starts_with("a="), "{text}");
        assert!(text.contains(" b="), "{text}");
        assert!(p.total() >= Duration::ZERO);
    }
}
