// SPDX-License-Identifier: GPL-3.0-or-later
import type { DockviewTheme } from "dockview-react";

/**
 * Pixel value of a design token (`--space-1` and so on), for APIs that only accept numbers.
 * Returns 0 where the token stylesheet is not loaded (unit tests).
 */
export function tokenPx(name: string): number {
  if (typeof document === "undefined") return 0;
  const raw = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const value = Number.parseFloat(raw);
  return Number.isFinite(value) ? value : 0;
}

/** Dockview theme: colours and shape come from `.dockview-theme-cutweave` in app.css (tokens only). */
export function dockTheme(): DockviewTheme {
  return {
    name: "cutweave",
    className: "dockview-theme-cutweave",
    gap: tokenPx("--space-1"),
    dndOverlayMounting: "absolute",
    dndPanelOverlay: "group",
  };
}
