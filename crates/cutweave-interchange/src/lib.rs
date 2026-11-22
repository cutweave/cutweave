// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-interchange`: OTIO, subtitle, FCPXML and EDL import and export.
//!
//! Stub crate created by WP-0A.01; real content arrives in later work packages.

/// Returns the crate name; placeholder until real content lands.
#[must_use]
pub const fn crate_name() -> &'static str {
    "cutweave-interchange"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "cutweave-interchange");
    }
}
