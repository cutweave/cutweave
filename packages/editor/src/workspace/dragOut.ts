// SPDX-License-Identifier: GPL-3.0-or-later
// Dragging a panel's tab out of the window (WP-2H.14, ADR-0031): dockview never opens a window from
// a drag, so a tab drag that ends with no drop, with the pointer outside every editor window, is
// read as "put it in its own window, here". Pure; the dock host wires it to dockview's tab drags.

/** A window's outer box on the screen, in CSS pixels (`screenX`, `screenY`, `outerWidth`...). */
export interface ScreenRect {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
}

/** The parts of a tab's `dragend` the decision reads. */
export interface DragEndLike {
  readonly screenX: number;
  readonly screenY: number;
  readonly dataTransfer: { readonly dropEffect: string } | null;
}

/** The outer box of `win` on the screen. */
export function screenRectOf(win: Window): ScreenRect {
  return { left: win.screenX, top: win.screenY, width: win.outerWidth, height: win.outerHeight };
}

/**
 * Where to open a new window for a tab whose drag ended like `end`, or `null` when it did not
 * leave the windows: it was dropped somewhere (the drop took it), the platform gave no position
 * (some Linux setups report 0, 0), or the pointer is still over one of `windows`.
 */
export function dragOutPoint(
  end: DragEndLike,
  windows: readonly ScreenRect[],
): { readonly x: number; readonly y: number } | null {
  if (end.dataTransfer !== null && end.dataTransfer.dropEffect !== "none") return null;
  const x = end.screenX;
  const y = end.screenY;
  if (!Number.isFinite(x) || !Number.isFinite(y) || (x === 0 && y === 0)) return null;
  const inside = windows.some(
    (r) => x >= r.left && x < r.left + r.width && y >= r.top && y < r.top + r.height,
  );
  return inside ? null : { x: Math.round(x), y: Math.round(y) };
}
