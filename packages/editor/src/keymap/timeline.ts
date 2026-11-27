// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline's keys as a context of the keymap (WP-2H.05, WP-2A.10): the app keymap's
// `timeline` context decides what a press is, and the interaction machine (WP-2A.08) is told
// which of its actions that is. A few timeline keys are the editor's own (markers, in and out,
// rename, duplicate): the machine never sees those as actions.
import type { MachineAction } from "@cutweave/timeline";
import { matchEvent } from "./store.ts";
import type { KeyEventLike } from "./stroke.ts";

/** The keymap command of each machine action. */
export const TIMELINE_COMMANDS: Readonly<Record<MachineAction, string>> = {
  cancel: "timeline.cancel",
  split: "clip.split",
  delete: "item.delete",
  ripple_delete: "timeline.ripple_delete",
  toggle_snapping: "timeline.toggle_snapping",
  nudge_left: "timeline.nudge_left",
  nudge_right: "timeline.nudge_right",
  nudge_left_10: "timeline.nudge_left_10",
  nudge_right_10: "timeline.nudge_right_10",
  select_all: "timeline.select_all",
  deselect: "timeline.deselect",
  "tool.select": "timeline.tool.select",
  "tool.ripple": "timeline.tool.ripple",
  "tool.roll": "timeline.tool.roll",
  "tool.slip": "timeline.tool.slip",
  "tool.slide": "timeline.tool.slide",
  "tool.razor": "timeline.tool.razor",
  "tool.hand": "timeline.tool.hand",
  "tool.zoom": "timeline.tool.zoom",
};

const ACTION_OF = new Map(
  (Object.entries(TIMELINE_COMMANDS) as [MachineAction, string][]).map(([action, id]) => [
    id,
    action,
  ]),
);

/** The timeline commands the editor runs itself, not the interaction machine. */
export const EDITOR_COMMANDS = [
  "item.duplicate",
  "timeline.add_marker",
  "timeline.mark_in",
  "timeline.mark_out",
  "timeline.match_frame",
  "track.rename",
] as const;
export type TimelineEditorCommand = (typeof EDITOR_COMMANDS)[number];

const EDITOR_SET: ReadonlySet<string> = new Set(EDITOR_COMMANDS);

/** What a key press in the timeline is: a machine action, an editor command, or nothing. */
export type TimelineKey =
  | { readonly kind: "action"; readonly action: MachineAction }
  | { readonly kind: "editor"; readonly command: TimelineEditorCommand }
  | { readonly kind: "none" };

/** What the keymap's `timeline` context makes of a key press. */
export function timelineKeyOf(e: KeyEventLike): TimelineKey {
  const binding = matchEvent(e, ["timeline"]);
  if (binding !== null) {
    const action = ACTION_OF.get(binding.command);
    if (action !== undefined) return { kind: "action", action };
    if (EDITOR_SET.has(binding.command)) {
      return { kind: "editor", command: binding.command as TimelineEditorCommand };
    }
  }
  // Esc always ends a drag, whatever the keymap says.
  if (e.key === "Escape" || e.key === "Esc") return { kind: "action", action: "cancel" };
  return { kind: "none" };
}

/** The machine action a key press is, or `null`. */
export function timelineActionOf(e: KeyEventLike): MachineAction | null {
  const key = timelineKeyOf(e);
  return key.kind === "action" ? key.action : null;
}

/** The editor-side timeline command a key press is, or `null`. */
export function timelineEditorCommandOf(e: KeyEventLike): TimelineEditorCommand | null {
  const key = timelineKeyOf(e);
  return key.kind === "editor" ? key.command : null;
}
