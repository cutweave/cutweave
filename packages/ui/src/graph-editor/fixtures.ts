// SPDX-License-Identifier: GPL-3.0-or-later
// Stories and tests only: curves evaluated by a stand-in for the engine's math (the editor
// passes the shared wasm module's evaluation), fixed tokens for a canvas without a stylesheet,
// and a canvas context that records what was drawn.
import type { CurveValue } from "../curve-editor/CurveEditor.tsx";
import { sampleCurve } from "../curve-editor/fixtures.ts";
import { lowerBound } from "./geometry.ts";
import type { GraphTheme } from "./theme.ts";
import type { GraphCurve, GraphOptions } from "./view.ts";

/** Time units per second in the fixtures (the editor uses flicks). */
export const SECOND = 1000;
export const FRAME = SECOND / 25;

export type FixtureKey = readonly [t: number, v: number, interp?: CurveValue];

const LINEAR: CurveValue = { kind: "linear" };

const tables = new WeakMap<CurveValue, Float64Array>();

function easedTable(interp: CurveValue): Float64Array {
  let table = tables.get(interp);
  if (table === undefined) {
    table = sampleCurve(interp, 257);
    tables.set(interp, table);
  }
  return table;
}

/** A curve through `keys`, each segment eased by its key's interpolation. */
export function makeCurve(
  id: string,
  label: string,
  color: number,
  keys: readonly FixtureKey[],
): GraphCurve {
  const times = Float64Array.from(keys, (k) => k[0]);
  const values = Float64Array.from(keys, (k) => k[1]);
  const interps = keys.map((k) => k[2] ?? LINEAR);
  const at = (t: number): number => {
    const n = times.length;
    if (n === 0) return 0;
    if (t <= (times[0] as number)) return values[0] as number;
    if (t >= (times[n - 1] as number)) return values[n - 1] as number;
    const i = lowerBound(times, t + 1e-9) - 1;
    const t0 = times[i] as number;
    const t1 = times[i + 1] as number;
    const u = (t - t0) / (t1 - t0);
    // 257 samples of the easing, read at the nearest one: close enough for a picture.
    const eased = easedTable(interps[i] as CurveValue);
    const p = eased[Math.round(u * 256)] as number;
    return (values[i] as number) + p * ((values[i + 1] as number) - (values[i] as number));
  };
  return {
    id,
    label,
    color,
    times,
    values,
    interps,
    sample(t0, t1, n, out) {
      for (let i = 0; i < n; i++) out[i] = at(t0 + ((t1 - t0) * i) / Math.max(1, n - 1));
    },
  };
}

export const EASE: CurveValue = { kind: "bezier", x1: 0.42, y1: 0, x2: 0.58, y2: 1 };

/** A position animation: three keys, two components, eased. */
export function positionCurves(): GraphCurve[] {
  return [
    makeCurve("position.x", "Position X", 0, [
      [1000, -400, EASE],
      [3000, 200, EASE],
      [5000, 0],
    ]),
    makeCurve("position.y", "Position Y", 1, [
      [1000, 0, EASE],
      [3000, -150, EASE],
      [5000, 300],
    ]),
  ];
}

/** An opacity fade with a hold and a spring. */
export function mixedCurves(): GraphCurve[] {
  return [
    makeCurve("opacity", "Opacity", 2, [
      [500, 0, { kind: "hold" }],
      [1500, 1, { kind: "spring", stiffness: 180, damping: 12, mass: 1, bounce: false }],
      [4000, 0.25, { kind: "bezier", x1: 0.34, y1: 1.56, x2: 0.64, y2: 1 }],
      [5500, 1],
    ]),
  ];
}

/** `curves` curves of `keys` keys each (the pan and zoom probe). */
export function denseCurves(curves: number, keys: number): GraphCurve[] {
  const out: GraphCurve[] = [];
  for (let c = 0; c < curves; c++) {
    const list: FixtureKey[] = [];
    for (let k = 0; k < keys; k++) {
      list.push([k * 200, Math.sin(k * 0.21 + c) * 100 + c * 20, k % 2 === 0 ? EASE : LINEAR]);
    }
    out.push(makeCurve(`curve.${c}`, `Curve ${c + 1}`, c, list));
  }
  return out;
}

const two = (n: number): string => String(Math.floor(n)).padStart(2, "0");

export const FIXTURE_OPTIONS: GraphOptions = {
  second: SECOND,
  frame: FRAME,
  formatTime: (t) => {
    const frames = Math.round(t / FRAME);
    const s = Math.floor(frames / 25);
    return `${two(s / 60)}:${two(s % 60)}:${two(frames % 25)}`;
  },
  formatValue: (v) => String(Number(v.toPrecision(6))),
};

/** Tokens for a canvas outside a themed document. */
export function fixtureTheme(): GraphTheme {
  return {
    surface: "var(--surface-0)",
    ruler: "var(--surface-1)",
    grid: "var(--border-subtle)",
    gridStrong: "var(--border-default)",
    text: "var(--text-muted)",
    textStrong: "var(--text-primary)",
    playhead: "var(--playhead)",
    selected: "var(--marigold)",
    onSelected: "var(--on-marigold)",
    marquee: "var(--marigold-subtle)",
    guide: "var(--overlay-guide)",
    curves: ["var(--label-blue)", "var(--label-orange)", "var(--label-green)"],
    font: "var(--font-mono)",
    fontPx: 12,
    dimmed: 0.5,
    line: 1,
    lineStrong: 2,
    rulerHeight: 24,
    rulerWidth: 64,
    keySize: 8,
    handleSize: 6,
    hitRadius: 8,
    pad: 4,
  };
}

/** A 2D context that draws nothing and counts the calls of each method. */
export function countingContext(): { ctx: CanvasRenderingContext2D; calls: Record<string, number> } {
  const calls: Record<string, number> = {};
  const ctx = new Proxy(
    {},
    {
      get: (_target, prop) => {
        if (typeof prop !== "string") return undefined;
        return (..._args: unknown[]) => {
          calls[prop] = (calls[prop] ?? 0) + 1;
          return prop === "measureText" ? { width: 0 } : undefined;
        };
      },
      set: () => true,
    },
  ) as unknown as CanvasRenderingContext2D;
  return { ctx, calls };
}
