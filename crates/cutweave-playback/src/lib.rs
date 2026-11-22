// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-playback`: transport, A/V sync, prefetch, scrub scheduler and shared-memory frame publisher.
//!
//! Stub crate created by WP-0A.01; real content arrives in later work packages.

// Justification: this crate talks to OS, FFI or real-time interfaces (docs/01-architecture.md §3)
// and is allowed to use `unsafe`; every use must carry a `// SAFETY:` comment.
#![allow(unsafe_code)]

/// Returns the crate name; placeholder until real content lands.
#[must_use]
pub const fn crate_name() -> &'static str {
    "cutweave-playback"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "cutweave-playback");
    }
}
