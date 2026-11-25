// SPDX-License-Identifier: GPL-3.0-or-later
//! Easing math (keyframes §4): the progress `p` of a segment, `dt` flicks into it.
//!
//! [`SegmentEasing`] holds what a segment's easing needs that does not depend on `dt` (the
//! Bézier polynomial coefficients, the spring's roots and its residual `x(D)`), so the
//! [`Evaluator`](super::Evaluator) can keep it between calls. One-off evaluation builds the same
//! value and calls the same code, so both return the same bits.

use super::interp::Interp;
use crate::FLICKS_PER_SECOND;

/// `|ζ − 1| ≤ 1e-9` uses the critically damped form (§4.5).
const CRITICAL_BAND: f64 = 1e-9;

/// Newton–Raphson steps before the bisection fallback (§4.4).
const NEWTON_STEPS: usize = 8;
/// Newton stops when a step is smaller than this.
const NEWTON_TOLERANCE: f64 = 1e-14;
/// Bisection narrows the bracket to this width (2⁻⁵²).
const BISECTION_WIDTH: f64 = f64::EPSILON;
/// Bisection never needs more halvings than this to reach [`BISECTION_WIDTH`] from [0, 1].
const BISECTION_STEPS: usize = 64;

/// `x as f64`, rounding to nearest-even: exact below 2⁵³ (§4.1, step 2).
#[allow(clippy::cast_precision_loss)]
fn to_f64(x: i64) -> f64 {
    x as f64
}

/// `B(s) = ((a·s + b)·s + c)·s` for one axis of a unit cubic Bézier, in Horner form (as WebKit's
/// `UnitBezier`, §4.4).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cubic {
    a: f64,
    b: f64,
    c: f64,
}

impl Cubic {
    /// The axis through (0, 0), control values `p1`, `p2`, and (1, 1).
    fn new(p1: f64, p2: f64) -> Cubic {
        let c = 3.0 * p1;
        let b = 3.0 * (p2 - p1) - c;
        let a = 1.0 - c - b;
        Cubic { a, b, c }
    }

    fn sample(self, s: f64) -> f64 {
        ((self.a * s + self.b) * s + self.c) * s
    }

    fn derivative(self, s: f64) -> f64 {
        (3.0 * self.a * s + 2.0 * self.b) * s + self.c
    }
}

/// `s ∈ [0, 1]` with `Bx(s) = u` (§4.4). With `x` handles in [0, 1], `Bx` is monotone
/// non-decreasing, so the solution is unique.
///
/// Newton–Raphson from `s = u` (at most 8 steps, stopping when a step is below 1e-14 inside
/// [0, 1]); otherwise bisection on [0, 1] down to a bracket of 2⁻⁵². Both loops have fixed caps,
/// so the run time never depends on anything but the inputs. Bisection never stops on a small
/// residual `|Bx(s) − u|`: near a vertical start that would give `p = 0` one flick in.
fn solve_x(x: Cubic, u: f64) -> f64 {
    let mut s = u;
    for _ in 0..NEWTON_STEPS {
        let slope = x.derivative(s);
        if !(slope.is_finite() && slope.abs() > 1e-12) {
            break;
        }
        let step = (x.sample(s) - u) / slope;
        s -= step;
        if !(0.0..=1.0).contains(&s) {
            break;
        }
        if step.abs() < NEWTON_TOLERANCE {
            return s;
        }
    }
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..BISECTION_STEPS {
        if hi - lo <= BISECTION_WIDTH {
            break;
        }
        let mid = f64::midpoint(lo, hi);
        if x.sample(mid) < u {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    f64::midpoint(lo, hi)
}

/// The solved form of a spring: `x(τ)`, the distance still to go after `τ` seconds, released
/// from rest at distance 1 (§4.5).
#[derive(Clone, Copy, Debug, PartialEq)]
enum SpringCurve {
    /// ζ < 1 − 1e-9: `e^(−ζω₀τ)·(cos(ω_d τ) + (ζω₀/ω_d)·sin(ω_d τ))`.
    Under {
        /// ζ·ω₀.
        decay: f64,
        /// ω_d = ω₀·√(1 − ζ²).
        wd: f64,
    },
    /// |ζ − 1| ≤ 1e-9: `e^(−ω₀τ)·(1 + ω₀τ)`.
    Critical {
        /// ω₀.
        w0: f64,
    },
    /// ζ > 1 + 1e-9: `(r₂·e^(r₁τ) − r₁·e^(r₂τ)) / (r₂ − r₁)`.
    Over {
        /// The slow root, −ω₀/(ζ + q), written to avoid cancellation.
        r1: f64,
        /// The fast root, −ω₀·(ζ + q).
        r2: f64,
    },
}

impl SpringCurve {
    fn new(stiffness: f64, damping: f64, mass: f64) -> SpringCurve {
        let w0 = (stiffness / mass).sqrt();
        let zeta = damping / (2.0 * (stiffness * mass).sqrt());
        if zeta < 1.0 - CRITICAL_BAND {
            SpringCurve::Under {
                decay: zeta * w0,
                wd: w0 * (1.0 - zeta * zeta).sqrt(),
            }
        } else if zeta <= 1.0 + CRITICAL_BAND {
            SpringCurve::Critical { w0 }
        } else {
            let q = (zeta * zeta - 1.0).sqrt();
            SpringCurve::Over {
                r1: -w0 / (zeta + q),
                r2: -w0 * (zeta + q),
            }
        }
    }

    /// `x(τ)` for `τ` in seconds.
    fn displacement(self, tau: f64) -> f64 {
        match self {
            SpringCurve::Under { decay, wd } => {
                libm::exp(-decay * tau) * (libm::cos(wd * tau) + (decay / wd) * libm::sin(wd * tau))
            }
            SpringCurve::Critical { w0 } => libm::exp(-w0 * tau) * (1.0 + w0 * tau),
            SpringCurve::Over { r1, r2 } => {
                (r2 * libm::exp(r1 * tau) - r1 * libm::exp(r2 * tau)) / (r2 - r1)
            }
        }
    }
}

/// Seconds from flicks, as one f64 division (§4.5: `τ = dt / F`).
fn seconds(flicks: i64) -> f64 {
    to_f64(flicks) / to_f64(FLICKS_PER_SECOND)
}

/// What a segment's easing needs, with everything that does not depend on `dt` precomputed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    /// `p = 0` (the value is the key's).
    Hold,
    /// `p = u`.
    Linear,
    /// `p = By(s)` where `Bx(s) = u`.
    Bezier { x: Cubic, y: Cubic },
    /// `p = 1 − x(τ) + u·x(D)`.
    Spring {
        curve: SpringCurve,
        bounce: bool,
        /// `x(D)`, the residual that lands the spring on the next key.
        residual: f64,
    },
}

/// The easing of one segment: its interp's shape for a span of `span` flicks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SegmentEasing {
    shape: Shape,
    span: i64,
}

impl SegmentEasing {
    /// The easing of a segment of `span` flicks starting at a key with `interp`. A segment of
    /// zero or negative length holds (keys strictly increase, so evaluation never builds one).
    pub(crate) fn new(interp: &Interp, span: i64) -> SegmentEasing {
        let shape = match *interp {
            _ if span <= 0 => Shape::Hold,
            Interp::Hold => Shape::Hold,
            Interp::Linear => Shape::Linear,
            Interp::Bezier { x1, y1, x2, y2, .. } => Shape::Bezier {
                // `x` is clamped defensively: outside [0, 1] Bx would not be monotone.
                x: Cubic::new(x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0)),
                y: Cubic::new(y1, y2),
            },
            Interp::Spring {
                stiffness,
                damping,
                mass,
                bounce,
                ..
            } => {
                let curve = SpringCurve::new(stiffness, damping, mass);
                Shape::Spring {
                    curve,
                    bounce,
                    residual: spring_x(curve, bounce, seconds(span)),
                }
            }
        };
        SegmentEasing { shape, span }
    }

    /// Whether `p` is always 0.
    pub(crate) fn is_hold(&self) -> bool {
        matches!(self.shape, Shape::Hold)
    }

    /// The progress `dt` flicks into the segment (§4.1).
    pub(crate) fn progress(&self, dt: i64) -> f64 {
        let u = || to_f64(dt) / to_f64(self.span);
        match self.shape {
            Shape::Hold => 0.0,
            Shape::Linear => u(),
            // The ends are exact: E(0) = 0, and u rounds to 1 only on spans beyond 2^53.
            Shape::Bezier { .. } if dt <= 0 => 0.0,
            Shape::Bezier { x, y } => match u() {
                u if u >= 1.0 => 1.0,
                u => y.sample(solve_x(x, u)),
            },
            Shape::Spring {
                curve,
                bounce,
                residual,
            } => 1.0 - spring_x(curve, bounce, seconds(dt)) + u() * residual,
        }
    }
}

/// `x(τ)`, reflected at the target when the spring bounces (§4.5: `x ← |x|`).
fn spring_x(curve: SpringCurve, bounce: bool, tau: f64) -> f64 {
    let x = curve.displacement(tau);
    if bounce { x.abs() } else { x }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_solver_handles_the_hard_cases() {
        // Steep start: Bx(s) = s³; one flick in, s ≈ u^(1/3), p ≈ 3s.
        let x = Cubic::new(0.0, 0.0);
        let u = 1.0 / 705_600_000.0;
        let s = solve_x(x, u);
        assert!((s - libm::cbrt(u)).abs() < 1e-15, "{s}");
        // Flat inflection at u = 0.5 for x1 = 1, x2 = 0.
        let x = Cubic::new(1.0, 0.0);
        for u in [0.25, 0.5 + 1e-6, 0.75, 1e-9, 1.0 - 1e-9] {
            let s = solve_x(x, u);
            assert!((x.sample(s) - u).abs() < 1e-12, "u = {u}");
        }
        // Linear handles: s = u.
        let x = Cubic::new(1.0 / 3.0, 2.0 / 3.0);
        assert!((solve_x(x, 0.3) - 0.3).abs() < 1e-15);
    }

    #[test]
    fn springs_land_on_the_target() {
        for (k, c, m, bounce) in [
            (170.0, 26.0, 1.0, false),
            (100.0, 0.0, 1.0, false),
            (100.0, 20.0, 1.0, false),
            (1.0, 100.0, 1.0, false),
            (160.0, 10.0, 1.0, true),
        ] {
            let interp = Interp::Spring {
                stiffness: k,
                damping: c,
                mass: m,
                bounce,
                preset: None,
            };
            let span = FLICKS_PER_SECOND / 3;
            let e = SegmentEasing::new(&interp, span);
            assert_eq!(e.progress(0), 0.0, "p(0) = 0 exactly");
            assert!((e.progress(span) - 1.0).abs() < 1e-12, "p(1) = 1");
        }
    }

    #[test]
    fn degenerate_spans_hold() {
        assert!(SegmentEasing::new(&Interp::Linear, 0).is_hold());
        assert!(SegmentEasing::new(&Interp::Linear, -5).is_hold());
        assert_eq!(Interp::Linear.progress(3, 0), 0.0);
    }
}
