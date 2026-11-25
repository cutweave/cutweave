// SPDX-License-Identifier: GPL-3.0-or-later
// The keys of the Preview's Source view (WP-1M.15) as one table: the panel reads it, and Settings
// > Keyboard shortcuts lists it (shortcuts/bindings.ts). Space, J/K/L and the arrows are the
// transport's own (transport/keymap.ts); they act on the source while it is shown.

export type SourceKeyAction = "back" | "insert";

export interface SourceKey {
  readonly action: SourceKeyAction;
  /** `KeyboardEvent.key`. */
  readonly key: string;
}

export const SOURCE_KEYS: readonly SourceKey[] = [
  { action: "back", key: "Escape" },
  { action: "insert", key: "Enter" },
];

/** What a plain key press does in the Source view, or `null` (a modifier makes it another key). */
export function sourceKeyAction(e: {
  readonly key: string;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
}): SourceKeyAction | null {
  if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return null;
  return SOURCE_KEYS.find((entry) => entry.key === e.key)?.action ?? null;
}
