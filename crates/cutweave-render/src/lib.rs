// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-render`: timeline evaluation, scene graph, render graph and wgpu passes (wasm-safe core).
//!
//! Stub crate created by WP-0A.01; real content arrives in later work packages.

/// Returns the crate name; placeholder until real content lands.
#[must_use]
pub const fn crate_name() -> &'static str {
    "cutweave-render"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "cutweave-render");
    }
}
