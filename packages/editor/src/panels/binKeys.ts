// SPDX-License-Identifier: GPL-3.0-or-later
// The media bin's keys (WP-1M.04, WP-1M.12, WP-2H.05): what a key press does in the bin is the
// app keymap's `bin` context (`keymap/`, Settings > Keyboard shortcuts), so a rebinding applies
// here and the page lists the same keys the panel reads. `import` works anywhere in the app.

import { matchEvent } from "../keymap/store.ts";
import type { KeyEventLike } from "../keymap/stroke.ts";

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
  | "search";

/** The keymap command of each bin action. */
export const BIN_COMMANDS: Readonly<Record<BinKeyAction, string>> = {
  moveNext: "bin.move_next",
  movePrevious: "bin.move_previous",
  moveDown: "bin.move_down",
  moveUp: "bin.move_up",
  first: "bin.first",
  last: "bin.last",
  insert: "bin.insert",
  play: "bin.play",
  remove: "media.remove",
  import: "bin.import",
  search: "bin.search",
};

const ACTION_OF = new Map(
  (Object.entries(BIN_COMMANDS) as [BinKeyAction, string][]).map(([action, id]) => [id, action]),
);

/** The slice of a `KeyboardEvent` the bin reads. */
export type BinKeyEvent = KeyEventLike;

function lookup(e: BinKeyEvent): BinKeyAction | null {
  const binding = matchEvent(e, ["bin", "global"]);
  return binding === null ? null : (ACTION_OF.get(binding.command) ?? null);
}

/**
 * What a key press does in the bin, or `null`. A plain key ignores Shift (Shift+Arrow still
 * moves); a Ctrl/Cmd key needs the modifier and nothing else.
 */
export function binKeyAction(e: BinKeyEvent): BinKeyAction | null {
  const found = lookup(e);
  if (found !== null) return found;
  if (e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey) return lookup({ ...e, shiftKey: false });
  return null;
}
