// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline's keys as a context of the keymap (WP-2H.05): `TIMELINE_KEYS` in the timeline
// package are its defaults, the app keymap's `timeline` context decides, and the interaction
// machine is told which action a press is.
import type { TimelineKeyAction } from "@cutweave/timeline";
import { matchEvent } from "./store.ts";
import type { KeyEventLike } from "./stroke.ts";

/** The keymap command of each timeline action. */
export const TIMELINE_COMMANDS: Readonly<Record<TimelineKeyAction, string>> = {
  cancel: "timeline.cancel",
  split: "clip.split",
  delete: "item.delete",
  "toggle-snapping": "timeline.toggle_snapping",
  "rename-track": "track.rename",
};

const ACTION_OF = new Map(
  (Object.entries(TIMELINE_COMMANDS) as [TimelineKeyAction, string][]).map(([action, id]) => [
    id,
    action,
  ]),
);

/** The timeline commands the editor runs itself (not the interaction machine): Duplicate. */
const EDITOR_COMMANDS: ReadonlySet<string> = new Set(["item.duplicate"]);

/** The editor-side timeline command a key press is (`item.duplicate`), or `null`. */
export function timelineEditorCommandOf(e: KeyEventLike): string | null {
  const binding = matchEvent(e, ["timeline"]);
  return binding !== null && EDITOR_COMMANDS.has(binding.command) ? binding.command : null;
}

/** The timeline action a key press is, or `null`. */
export function timelineActionOf(e: KeyEventLike): TimelineKeyAction | null {
  const binding = matchEvent(e, ["timeline"]);
  return binding === null ? null : (ACTION_OF.get(binding.command) ?? null);
}
