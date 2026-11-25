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
  | "search"
  | "up"
  | "rename"
  | "selectAll"
  | "newFolder";

/** The keymap command of each bin action. */
export const BIN_COMMANDS: Readonly<Record<Exclude<BinKeyAction, "selectAll">, string>> = {
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
  up: "bin.up",
  rename: "bin.rename",
  newFolder: "bin.new_folder",
};

// Ctrl/Cmd+A is the fixed Select all of text fields (`edit.text.select_all`, reserved), so the bin
// does not bind it again: in the bin, away from a text field, the same key selects every tile.
const ACTION_OF = new Map<string, BinKeyAction>([
  ...(Object.entries(BIN_COMMANDS) as [BinKeyAction, string][]).map(
    ([action, id]): [string, BinKeyAction] => [id, action],
  ),
  ["edit.text.select_all", "selectAll"],
]);

/** The slice of a `KeyboardEvent` the bin reads. */
export type BinKeyEvent = KeyEventLike;

function lookup(e: BinKeyEvent): BinKeyAction | null {
  if (typeof e.key !== "string") return null;
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
