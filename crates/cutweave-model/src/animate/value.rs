// SPDX-License-Identifier: GPL-3.0-or-later
//! Value types that can be keyframed, and how each one interpolates (keyframes §5).

use std::fmt;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::props::ValueRange;
use crate::{Color, Vec2};

/// A value type that can be keyframed (keyframes §5).
///
/// Every interpolating kind is an easing `p` fed into one [`combine`](Self::combine) per value
/// type, so presets and curves are shared by every property. Evaluation works on the
/// **evaluated form** ([`Eval`](Self::Eval)), which equals the stored type except for colours.
pub trait AnimValue:
    Clone + PartialEq + fmt::Debug + Serialize + DeserializeOwned + Send + Sync + 'static
{
    /// What evaluation returns (keyframes §2): the stored type, or premultiplied linear-light
    /// RGBA for colours (§5.4).
    type Eval: Clone + PartialEq + fmt::Debug + Send + Sync + 'static;

    /// Booleans and enums: they only hold (KF-6, §5.5).
    const DISCRETE: bool = false;

    /// The stored value in its evaluated form.
    fn to_eval(&self) -> Self::Eval;

    /// An evaluated value back in the stored form (a key set without an explicit value, §9.2).
    /// The identity except for colours (§5.4).
    fn from_eval(value: &Self::Eval) -> Self;

    /// The value at progress `p` between `a` (`p = 0`) and `b` (`p = 1`), §5. Discrete types are
    /// never combined (evaluation returns the key's value) and return `a`.
    fn combine(a: &Self::Eval, b: &Self::Eval, p: f64) -> Self::Eval;

    /// `value` with every non-finite component replaced by `fallback`'s (§6, step 6).
    fn sanitize(value: Self::Eval, fallback: &Self::Eval) -> Self::Eval;

    /// Whether the value is a valid stored value within `range` (KF-4): numbers finite and in
    /// range, per component.
    fn fits(&self, range: ValueRange) -> bool;

    /// The property's evaluation clamp (§7), per component. Types without numbers ignore it.
    fn clamp(value: Self::Eval, range: ValueRange) -> Self::Eval {
        let _ = range;
        value
    }
}

/// `a + (b − a)·p`, in exactly this form (§5.1): monotone in `p` for IEEE arithmetic, and
/// bit-identical on every platform (no fused multiply-add: Rust never contracts).
#[must_use]
pub(crate) fn lerp(a: f64, b: f64, p: f64) -> f64 {
    a + (b - a) * p
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

/// Scalars (`opacity`, crop edges, `audio_gain`, `audio_pan`) and accumulating angles
/// (`transform.rotation`, §5.3: 0 → 720 is two turns).
impl AnimValue for f64 {
    type Eval = f64;

    fn to_eval(&self) -> f64 {
        *self
    }

    fn from_eval(value: &f64) -> f64 {
        *value
    }

    fn combine(a: &f64, b: &f64, p: f64) -> f64 {
        lerp(*a, *b, p)
    }

    fn sanitize(value: f64, fallback: &f64) -> f64 {
        finite_or(value, *fallback)
    }

    fn fits(&self, range: ValueRange) -> bool {
        range.contains(*self)
    }

    fn clamp(value: f64, range: ValueRange) -> f64 {
        range.clamp(value)
    }
}

/// Positions, scales and anchors: both components with the same `p` (linked easing, §5.2).
impl AnimValue for Vec2 {
    type Eval = Vec2;

    fn to_eval(&self) -> Vec2 {
        *self
    }

    fn from_eval(value: &Vec2) -> Vec2 {
        *value
    }

    fn combine(a: &Vec2, b: &Vec2, p: f64) -> Vec2 {
        Vec2::new(lerp(a.x, b.x, p), lerp(a.y, b.y, p))
    }

    fn sanitize(value: Vec2, fallback: &Vec2) -> Vec2 {
        Vec2::new(
            finite_or(value.x, fallback.x),
            finite_or(value.y, fallback.y),
        )
    }

    fn fits(&self, range: ValueRange) -> bool {
        range.contains(self.x) && range.contains(self.y)
    }

    fn clamp(value: Vec2, range: ValueRange) -> Vec2 {
        Vec2::new(range.clamp(value.x), range.clamp(value.y))
    }
}

/// Flips: discrete, so they hold (§5.5).
impl AnimValue for bool {
    type Eval = bool;
    const DISCRETE: bool = true;

    fn to_eval(&self) -> bool {
        *self
    }

    fn from_eval(value: &bool) -> bool {
        *value
    }

    fn combine(a: &bool, _: &bool, _: f64) -> bool {
        *a
    }

    fn sanitize(value: bool, _: &bool) -> bool {
        value
    }

    fn fits(&self, _: ValueRange) -> bool {
        true
    }
}

/// An angle in degrees that interpolates the **shortest way** round (§5.3): with
/// `d = v₁ − v₀` reduced to `[−180, 180)`, the value is `v₀ + d·p`. Exactly opposite angles turn
/// counter-clockwise (−180). The result is not wrapped into `[0, 360)`.
///
/// A registry attribute for later properties such as hue; no P2 property uses it (rotation
/// accumulates and is a plain `f64`). On the wire it is a bare number.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShortestAngle(pub f64);

impl AnimValue for ShortestAngle {
    type Eval = f64;

    fn to_eval(&self) -> f64 {
        self.0
    }

    fn from_eval(value: &f64) -> ShortestAngle {
        ShortestAngle(*value)
    }

    fn combine(a: &f64, b: &f64, p: f64) -> f64 {
        let d = b - a;
        let d = d - 360.0 * ((d + 180.0) / 360.0).floor();
        a + d * p
    }

    fn sanitize(value: f64, fallback: &f64) -> f64 {
        finite_or(value, *fallback)
    }

    fn fits(&self, range: ValueRange) -> bool {
        range.contains(self.0)
    }

    fn clamp(value: f64, range: ValueRange) -> f64 {
        range.clamp(value)
    }
}

/// A colour in the compositor's working space (arch §8): linear light, **premultiplied** alpha,
/// in `f64`. The evaluated form of [`Color`] (keyframes §5.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LinearRgba {
    /// Red, premultiplied, in `[0, a]`.
    pub r: f64,
    /// Green, premultiplied, in `[0, a]`.
    pub g: f64,
    /// Blue, premultiplied, in `[0, a]`.
    pub b: f64,
    /// Alpha, in `[0, 1]`.
    pub a: f64,
}

/// The sRGB EOTF: an encoded channel in `[0, 1]` to linear light.
fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        libm::pow((c + 0.055) / 1.055, 2.4)
    }
}

/// The sRGB OETF: linear light in `[0, 1]` to an encoded channel.
fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * libm::pow(c, 1.0 / 2.4) - 0.055
    }
}

/// `x` (in `[0, 1]`) to a byte: ×255, rounded half to even, clamped.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to [0, 255] first
fn to_byte(x: f64) -> u8 {
    let v = (x * 255.0).round_ties_even();
    if v >= 255.0 {
        255
    } else if v > 0.0 {
        v as u8
    } else {
        0
    }
}

/// Text and effect colours (P3/P4; no P2 property is a colour). Interpolated in linear light,
/// premultiplied, so a fade from transparent white to red shows no white fringe (§5.4).
impl AnimValue for Color {
    type Eval = LinearRgba;

    fn to_eval(&self) -> LinearRgba {
        let a = f64::from(self.a) / 255.0;
        let channel = |c: u8| srgb_to_linear(f64::from(c) / 255.0) * a;
        LinearRgba {
            r: channel(self.r),
            g: channel(self.g),
            b: channel(self.b),
            a,
        }
    }

    /// Un-premultiplies (0 when alpha is 0), applies the sRGB OETF, and rounds each channel to
    /// a byte, half to even (§5.4).
    fn from_eval(value: &LinearRgba) -> Color {
        let a = value.a.clamp(0.0, 1.0);
        let channel = |c: f64| {
            let straight = if a > 0.0 {
                (c / a).clamp(0.0, 1.0)
            } else {
                0.0
            };
            to_byte(linear_to_srgb(straight))
        };
        Color {
            r: channel(value.r),
            g: channel(value.g),
            b: channel(value.b),
            a: to_byte(a),
        }
    }

    /// Each premultiplied channel as a scalar with the same `p`; then alpha is clamped to
    /// `[0, 1]` and each colour channel to `[0, alpha]` (§5.4, step 4).
    fn combine(x: &LinearRgba, y: &LinearRgba, p: f64) -> LinearRgba {
        let a = lerp(x.a, y.a, p).clamp(0.0, 1.0);
        let channel = |c0: f64, c1: f64| lerp(c0, c1, p).max(0.0).min(a);
        LinearRgba {
            r: channel(x.r, y.r),
            g: channel(x.g, y.g),
            b: channel(x.b, y.b),
            a,
        }
    }

    fn sanitize(value: LinearRgba, fallback: &LinearRgba) -> LinearRgba {
        LinearRgba {
            r: finite_or(value.r, fallback.r),
            g: finite_or(value.g, fallback.g),
            b: finite_or(value.b, fallback.b),
            a: finite_or(value.a, fallback.a),
        }
    }

    fn fits(&self, _: ValueRange) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_is_exact_at_the_ends_of_simple_ranges() {
        assert_eq!(lerp(0.0, 1.0, 0.25), 0.25);
        assert_eq!(lerp(2.0, 4.0, 1.0), 4.0);
        // Overflowing differences give infinity, which sanitizing replaces.
        assert!(lerp(-1.5e308, 1.5e308, 0.5).is_infinite());
        assert_eq!(f64::sanitize(f64::INFINITY, &-1.5e308), -1.5e308);
    }

    #[test]
    fn shortest_angle_reduces_to_half_open_range() {
        let mid = |a: f64, b: f64| ShortestAngle::combine(&a, &b, 0.5);
        assert_eq!(mid(350.0, 10.0), 360.0);
        assert_eq!(mid(10.0, 350.0), 0.0);
        assert_eq!(
            mid(0.0, 180.0),
            -90.0,
            "exactly opposite turns counter-clockwise"
        );
        assert_eq!(mid(-720.0, 30.0), -705.0);
    }

    #[test]
    fn colours_round_trip_through_the_evaluated_form() {
        for hex in [
            "#000000",
            "#ffffff",
            "#ff000080",
            "#12345678",
            "#00000000",
            "#7f7f7f",
        ] {
            let c: Color = hex.parse().unwrap();
            let back = Color::from_eval(&c.to_eval());
            if c.a == 0 {
                assert_eq!(
                    back,
                    Color {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 0
                    },
                    "{hex}"
                );
            } else {
                assert_eq!(back, c, "{hex}");
            }
        }
    }

    #[test]
    fn colour_combine_clamps_to_premultiplied_range() {
        let black = Color::rgb(0, 0, 0).to_eval();
        let white = Color::rgb(255, 255, 255).to_eval();
        let over = Color::combine(&black, &white, 1.5);
        assert_eq!(
            over,
            LinearRgba {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0
            }
        );
        let clear = Color {
            r: 255,
            g: 255,
            b: 255,
            a: 0,
        }
        .to_eval();
        assert_eq!(
            clear,
            LinearRgba::default(),
            "transparent white is all zeros"
        );
    }

    #[test]
    fn ranges_and_clamps_apply_per_component() {
        let unit = ValueRange::new(0.0, 1.0);
        assert!(Vec2::new(0.0, 1.0).fits(unit));
        assert!(!Vec2::new(0.0, 1.5).fits(unit));
        assert!(!Vec2::new(f64::NAN, 0.5).fits(ValueRange::ANY));
        assert_eq!(Vec2::clamp(Vec2::new(-0.5, 2.0), unit), Vec2::new(0.0, 1.0));
        assert!(true.fits(unit));
        assert!(!f64::INFINITY.fits(ValueRange::ANY));
    }
}
