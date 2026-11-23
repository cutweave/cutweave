// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-engine`: composition root wiring the engine together and session lifecycle.
//!
//! Currently hosts [`project_io`] (WP-1C.01): the project folder format, atomic save/open and the
//! single-writer lock (arch §17, ADR-0006); [`journal`] (WP-1C.02): the append-only command
//! journal written before every acknowledgement, and crash recovery from it; and [`ui_state`]
//! (WP-1C.04), which persists the `view` namespace state. The rest of the composition root
//! arrives in later work packages.

pub mod journal;
pub mod project_io;
pub mod ui_state;

/// Returns the crate name.
#[must_use]
pub const fn crate_name() -> &'static str {
    "cutweave-engine"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "cutweave-engine");
    }
}
