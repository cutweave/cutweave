// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-tokens`: design tokens from `design/tokens.json` as `const` values (wasm-safe).
//!
//! The values live in `generated.rs`, written by `cargo xtask tokens`; never edit it by hand.
//! Colors are given per theme as sRGB `u8` and as linear-light `f32` (alpha is never
//! linearised); spacing and radii are CSS pixels.

#![no_std]

#[rustfmt::skip]
mod generated;

pub use generated::{dark, light, radius, spacing};

/// A color token value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    /// sRGB channels `[r, g, b, a]`, straight alpha.
    pub srgb: [u8; 4],
    /// Linear-light channels `[r, g, b, a]`; rgb are linearised, alpha is `a / 255`.
    pub linear: [f32; 4],
}

/// The color themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// The default, reference theme.
    Dark,
    /// The light theme.
    Light,
}

impl Theme {
    /// Every color of the theme as `(token name, color)`, in token-file order.
    #[must_use]
    pub const fn colors(self) -> &'static [(&'static str, Color)] {
        match self {
            Self::Dark => dark::ALL,
            Self::Light => light::ALL,
        }
    }

    /// Looks a color up by token name, e.g. `"surface-0"`.
    #[must_use]
    pub fn color(self, name: &str) -> Option<Color> {
        self.colors()
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, c)| *c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_0_matches_tokens_json() {
        assert_eq!(dark::SURFACE_0.srgb, [0x11, 0x11, 0x11, 0xff]);
        assert_eq!(light::SURFACE_0.srgb, [0xe7, 0xe7, 0xe7, 0xff]);
    }

    #[test]
    fn linear_values_follow_the_srgb_transfer_function() {
        let c = dark::MARIGOLD;
        assert!((c.linear[0] - 0.8879).abs() < 1e-3, "{}", c.linear[0]);
        assert!((dark::SURFACE_0.linear[0] - 0.005_605).abs() < 1e-5);
        assert!((c.linear[3] - 1.0).abs() < f32::EPSILON);
        // Alpha is kept linear: 0x24 / 255.
        assert!((dark::MARIGOLD_SUBTLE.linear[3] - 36.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn aliases_resolve_to_their_target() {
        assert_eq!(dark::PLAYHEAD, dark::MARIGOLD);
        assert_eq!(light::PLAYHEAD, light::MARIGOLD);
    }

    #[test]
    fn lookup_by_name_and_scales() {
        assert_eq!(Theme::Dark.color("surface-1"), Some(dark::SURFACE_1));
        assert_eq!(Theme::Light.color("nope"), None);
        assert!((spacing::SPACE_1 - 4.0).abs() < f32::EPSILON);
        assert!((radius::RADIUS_SM - 4.0).abs() < f32::EPSILON);
    }
}
