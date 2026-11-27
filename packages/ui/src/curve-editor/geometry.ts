// SPDX-License-Identifier: GPL-3.0-or-later
// The curve editor's graph geometry: time runs left to right from the key (0) to the next key
// (1); progress runs bottom to top from the key's value (0) to the next key's (1), with room
// above and below for curves that overshoot.

/** The graph's own units (the SVG `viewBox`); CSS scales it to the room it gets. */
export const PLOT = { width: 240, height: 200, pad: 14 } as const;
/** The progress range the graph shows (the engine accepts handles from -10 to 10). */
export const Y_MIN = -0.75;
export const Y_MAX = 1.75;
/** What one arrow key moves a handle by (Shift: ten times). */
export const HANDLE_STEP = 0.01;

export const toX = (u: number): number => PLOT.pad + u * (PLOT.width - 2 * PLOT.pad);
export const toY = (p: number): number =>
  PLOT.pad + ((Y_MAX - p) / (Y_MAX - Y_MIN)) * (PLOT.height - 2 * PLOT.pad);

/** A handle kept where the engine and the graph accept it: time in 0..1, progress in view. */
export function clampHandle(x: number, y: number): { x: number; y: number } {
  return {
    x: Math.min(1, Math.max(0, x)),
    y: Math.min(Y_MAX, Math.max(Y_MIN, y)),
  };
}

/** The (time, progress) under a pointer, given the graph's box on screen. */
export function pointFromClient(
  box: { left: number; top: number; width: number; height: number },
  clientX: number,
  clientY: number,
): { x: number; y: number } {
  const sx = box.width > 0 ? PLOT.width / box.width : 1;
  const sy = box.height > 0 ? PLOT.height / box.height : 1;
  const gx = (clientX - box.left) * sx;
  const gy = (clientY - box.top) * sy;
  const x = (gx - PLOT.pad) / (PLOT.width - 2 * PLOT.pad);
  const y = Y_MAX - ((gy - PLOT.pad) / (PLOT.height - 2 * PLOT.pad)) * (Y_MAX - Y_MIN);
  return { x: Math.round(x * 1000) / 1000, y: Math.round(y * 1000) / 1000 };
}

/** The SVG path through `samples` (progress at evenly spaced times from 0 to 1). */
export function curvePath(samples: ArrayLike<number>): string {
  const n = samples.length;
  if (n === 0) return "";
  let d = "";
  for (let i = 0; i < n; i++) {
    const u = n === 1 ? 0 : i / (n - 1);
    const p = Math.min(Y_MAX, Math.max(Y_MIN, samples[i] as number));
    d += `${i === 0 ? "M" : "L"}${toX(u).toFixed(2)} ${toY(Number.isFinite(p) ? p : 0).toFixed(2)}`;
  }
  return d;
}
