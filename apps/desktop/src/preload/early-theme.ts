// SPDX-License-Identifier: GPL-3.0-or-later
// The theme before the first paint (WP-1M.11). Main reads the preferences before it creates the
// window and answers the preload's one synchronous call with the theme to show; the preload sets
// `data-theme` on the root element the moment it exists, which is before the page's own scripts
// run, so a light window never flashes dark. The editor then takes over (it renders only after
// it has the preferences snapshot, so nothing flips back).
//
// Kept free of `electron` and of the real DOM so it is unit-testable.

export type EarlyTheme = "dark" | "light";

export function isEarlyTheme(value: unknown): value is EarlyTheme {
  return value === "dark" || value === "light";
}

/** The parts of `document` this uses. */
export interface ThemeTarget {
  readonly documentElement: { dataset: Record<string, string | undefined> } | null;
}

/**
 * Sets `data-theme` now when the root element exists, else as soon as it does (`whenReady`
 * calls back once, on the first change to the document). Anything but `dark` or `light` is
 * ignored: the page keeps its default.
 */
export function applyEarlyTheme(
  value: unknown,
  target: ThemeTarget,
  whenReady: (callback: () => void) => void,
): void {
  if (!isEarlyTheme(value)) return;
  const apply = (): boolean => {
    const root = target.documentElement;
    if (root === null) return false;
    root.dataset.theme = value;
    return true;
  };
  if (!apply()) whenReady(apply);
}
