// SPDX-License-Identifier: GPL-3.0-or-later
// The keys of the Preview's Source view (WP-1M.15, WP-2H.05, WP-2A.14): the app keymap's `source`
// context (`keymap/`, Settings > Keyboard shortcuts) decides, so the panel and the page list the
// same keys. Space, J/K/L and the arrows are the transport's own (transport/keymap.ts); they act
// on the source while it is shown. I and O mark the source here and the timeline's range there:
// the same keys, by focus.

import { matchEvent } from "../keymap/store.ts";
import type { KeyEventLike } from "../keymap/stroke.ts";

export type SourceKeyAction = "back" | "insert" | "overwrite" | "markIn" | "markOut" | "clearMarks";

/** The keymap command of each action. */
export const SOURCE_COMMANDS: Readonly<Record<SourceKeyAction, string>> = {
  back: "source.back",
  insert: "source.insert",
  overwrite: "source.overwrite",
  markIn: "source.mark_in",
  markOut: "source.mark_out",
  clearMarks: "source.clear_marks",
};

const ACTION_OF = new Map(
  (Object.entries(SOURCE_COMMANDS) as [SourceKeyAction, string][]).map(([action, id]) => [
    id,
    action,
  ]),
);

/** What a key press does in the Source view, or `null`. */
export function sourceKeyAction(e: KeyEventLike): SourceKeyAction | null {
  const binding = matchEvent(e, ["source"]);
  return binding === null ? null : (ACTION_OF.get(binding.command) ?? null);
}
