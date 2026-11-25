// SPDX-License-Identifier: GPL-3.0-or-later
// The keys of the Preview's Source view (WP-1M.15, WP-2H.05): the app keymap's `source` context
// (`keymap/`, Settings > Keyboard shortcuts) decides, so the panel and the page list the same
// keys. Space, J/K/L and the arrows are the transport's own (transport/keymap.ts); they act on
// the source while it is shown.

import { matchEvent } from "../keymap/store.ts";
import type { KeyEventLike } from "../keymap/stroke.ts";

export type SourceKeyAction = "back" | "insert";

/** The keymap command of each action. */
export const SOURCE_COMMANDS: Readonly<Record<SourceKeyAction, string>> = {
  back: "source.back",
  insert: "source.insert",
};

/** What a key press does in the Source view, or `null`. */
export function sourceKeyAction(e: KeyEventLike): SourceKeyAction | null {
  const binding = matchEvent(e, ["source"]);
  if (binding === null) return null;
  return binding.command === SOURCE_COMMANDS.back
    ? "back"
    : binding.command === SOURCE_COMMANDS.insert
      ? "insert"
      : null;
}
