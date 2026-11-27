// SPDX-License-Identifier: GPL-3.0-or-later
// Stories and tests only: a stand-in for the engine's easing math (the editor passes the shared
// wasm module's `easing.sample`), and a few of the engine's presets.
import type { CurvePreset, CurveValue } from "./CurveEditor.tsx";

/** Progress of a CSS-style cubic bezier at time `u`, by bisection on its x polynomial. */
function bezierAt(x1: number, y1: number, x2: number, y2: number, u: number): number {
  const at = (a: number, b: number, t: number): number =>
    3 * a * t * (1 - t) ** 2 + 3 * b * t * t * (1 - t) + t ** 3;
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 40; i++) {
    const mid = (lo + hi) / 2;
    if (at(x1, x2, mid) < u) lo = mid;
    else hi = mid;
  }
  return at(y1, y2, (lo + hi) / 2);
}

/** An underdamped spring over one second, released from rest (close enough for a picture). */
function springAt(stiffness: number, damping: number, mass: number, u: number): number {
  const w0 = Math.sqrt(stiffness / mass);
  const zeta = damping / (2 * Math.sqrt(stiffness * mass));
  if (zeta >= 1) return 1 - Math.exp(-w0 * u) * (1 + w0 * u);
  const wd = w0 * Math.sqrt(1 - zeta * zeta);
  const decay = Math.exp(-zeta * w0 * u);
  return 1 - decay * (Math.cos(wd * u) + ((zeta * w0) / wd) * Math.sin(wd * u));
}

export function sampleCurve(value: CurveValue, n: number): Float64Array {
  const out = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    const u = n === 1 ? 0 : i / (n - 1);
    if (value.kind === "hold") out[i] = u >= 1 ? 1 : 0;
    else if (value.kind === "linear") out[i] = u;
    else if (value.kind === "bezier") {
      out[i] = bezierAt(value.x1, value.y1, value.x2, value.y2, u);
    } else out[i] = springAt(value.stiffness, value.damping, value.mass, u);
  }
  return out;
}

const bezier = (id: string, x1: number, y1: number, x2: number, y2: number): CurveValue => ({
  kind: "bezier",
  x1,
  y1,
  x2,
  y2,
  preset: id,
});

export const SAMPLE_PRESETS: readonly CurvePreset[] = [
  { id: "hold", label: "Hold", value: { kind: "hold" } },
  { id: "linear", label: "Linear", value: { kind: "linear" } },
  { id: "ease_in", label: "Ease in", value: bezier("ease_in", 0.42, 0, 1, 1) },
  { id: "ease_out", label: "Ease out", value: bezier("ease_out", 0, 0, 0.58, 1) },
  { id: "ease_in_out", label: "Ease in-out", value: bezier("ease_in_out", 0.42, 0, 0.58, 1) },
  { id: "back_out", label: "Back out", value: bezier("back_out", 0.34, 1.56, 0.64, 1) },
  {
    id: "spring",
    label: "Spring",
    value: {
      kind: "spring",
      stiffness: 170,
      damping: 26,
      mass: 1,
      bounce: false,
      preset: "spring",
    },
  },
  {
    id: "spring_wobbly",
    label: "Spring, wobbly",
    value: {
      kind: "spring",
      stiffness: 180,
      damping: 12,
      mass: 1,
      bounce: false,
      preset: "spring_wobbly",
    },
  },
];
