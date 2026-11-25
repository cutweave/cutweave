// SPDX-License-Identifier: GPL-3.0-or-later
// The media bin's keys (WP-1M.04, WP-1M.12) as one table: the panel reads it to decide what a key
// press does, and Settings > Keyboard shortcuts lists it (WP-1M.13), so the two cannot drift.
// `key` is `KeyboardEvent.key`; `mod` means Ctrl, or Cmd on macOS.

export type BinKeyAction =
  | "moveNext"
  | "movePrevious"
  | "moveDown"
  | "moveUp"
  | "first"
  | "last"
  | "insert"
  | "play"
  | "remove"
  | "import"
  | "search"
  | "up"
  | "rename"
  | "selectAll"
  | "newFolder";

export interface BinKey {
  readonly action: BinKeyAction;
  readonly key: string;
  readonly mod?: boolean;
  /** The key needs Alt (Alt+Up goes up a folder). */
  readonly alt?: boolean;
  /** The key needs Shift (a plain key ignores it; Ctrl/Cmd+Shift+N is not Ctrl/Cmd+N). */
  readonly shift?: boolean;
  /** Works while the bin has the focus; otherwise anywhere in the app. */
  readonly inPanel: boolean;
}

export const BIN_KEYS: readonly BinKey[] = [
  { action: "moveNext", key: "ArrowRight", inPanel: true },
  { action: "movePrevious", key: "ArrowLeft", inPanel: true },
  { action: "moveDown", key: "ArrowDown", inPanel: true },
  { action: "moveUp", key: "ArrowUp", inPanel: true },
  { action: "first", key: "Home", inPanel: true },
  { action: "last", key: "End", inPanel: true },
  { action: "insert", key: "Enter", inPanel: true },
  { action: "play", key: " ", inPanel: true },
  { action: "remove", key: "Delete", inPanel: true },
  { action: "up", key: "Backspace", inPanel: true },
  { action: "up", key: "ArrowUp", alt: true, inPanel: true },
  { action: "rename", key: "F2", inPanel: true },
  { action: "selectAll", key: "a", mod: true, inPanel: true },
  { action: "newFolder", key: "n", mod: true, shift: true, inPanel: true },
  { action: "import", key: "i", mod: true, inPanel: false },
  { action: "search", key: "f", mod: true, inPanel: true },
];

/** The slice of a `KeyboardEvent` the bin reads. */
export interface BinKeyEvent {
  readonly key: string;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
}

/**
 * What a key press does in the bin, or `null`. Plain keys ignore Shift (Shift+Arrow still
 * moves); a Ctrl/Cmd key needs the modifier, and Shift only where the table says so; Alt only
 * where the table says so.
 */
export function binKeyAction(e: BinKeyEvent): BinKeyAction | null {
  const mod = e.ctrlKey || e.metaKey;
  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  for (const entry of BIN_KEYS) {
    if (entry.key !== key) continue;
    if ((entry.alt === true) !== e.altKey) continue;
    if (entry.mod === true) {
      if (!mod || (entry.shift === true) !== e.shiftKey) continue;
    } else if (mod || (entry.shift === true && !e.shiftKey)) {
      continue;
    }
    return entry.action;
  }
  return null;
}
