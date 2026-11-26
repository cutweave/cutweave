// SPDX-License-Identifier: GPL-3.0-or-later
// Tiny window-state persistence (bounds + maximized), with no dependency.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

export interface Bounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface WindowState {
  bounds: Bounds;
  maximized: boolean;
}

export const DEFAULT_WINDOW_STATE: WindowState = {
  bounds: { x: 0, y: 0, width: 1440, height: 900 },
  maximized: false,
};

export const MIN_WINDOW = { width: 960, height: 600 } as const;

const isFiniteNumber = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

/** Parse persisted JSON; anything malformed yields null. */
export function parseWindowState(raw: string): WindowState | null {
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof data !== "object" || data === null) return null;
  const { bounds, maximized } = data as { bounds?: Partial<Bounds>; maximized?: unknown };
  if (
    !bounds ||
    !isFiniteNumber(bounds.x) ||
    !isFiniteNumber(bounds.y) ||
    !isFiniteNumber(bounds.width) ||
    !isFiniteNumber(bounds.height)
  ) {
    return null;
  }
  return {
    bounds: { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height },
    maximized: maximized === true,
  };
}

/**
 * Keep a restored window reachable: enforce the minimum size (the editor's, unless `min` says
 * otherwise: a detached panel's window is smaller) and require it to overlap one of the current
 * displays; otherwise drop the position (the OS centers the window) but keep the size.
 */
export function fitToDisplays(
  state: WindowState,
  displays: readonly Bounds[],
  min: { readonly width: number; readonly height: number } = MIN_WINDOW,
): { bounds: Partial<Bounds> & Pick<Bounds, "width" | "height">; maximized: boolean } {
  const width = Math.max(state.bounds.width, min.width);
  const height = Math.max(state.bounds.height, min.height);
  const { x, y } = state.bounds;
  const visible = displays.some(
    (d) =>
      x < d.x + d.width - 40 &&
      x + width > d.x + 40 &&
      y < d.y + d.height - 40 &&
      y + height > d.y + 40,
  );
  return {
    bounds: visible ? { x, y, width, height } : { width, height },
    maximized: state.maximized,
  };
}

export function loadWindowState(file: string): WindowState | null {
  try {
    return parseWindowState(readFileSync(file, "utf8"));
  } catch {
    return null;
  }
}

export function saveWindowState(file: string, state: WindowState): void {
  try {
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, JSON.stringify(state), "utf8");
  } catch {
    // Persistence is best effort; never block quitting on it.
  }
}
