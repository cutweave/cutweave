// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-media`: MediaBackend trait and FFmpeg implementation (probe, index, decode, encode, mux, HW accel).
//!
//! Stub crate created by WP-0A.01; real content arrives in later work packages.

/// Returns the crate name; placeholder until real content lands.
#[must_use]
pub const fn crate_name() -> &'static str {
    "cutweave-media"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "cutweave-media");
    }
}
