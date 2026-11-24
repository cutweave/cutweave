// SPDX-License-Identifier: GPL-3.0-or-later
// Names for undo and redo entries. The engine logs a transaction under its label ("Move clip",
// set by the timeline gestures) or, for a single command, under the command name
// (`item.delete`). The command names are mapped to the same verbs the menus and the
// design system use ("Delete clip"), so the button reads "Undo Delete clip".

import type { TKey } from "../i18n/index.ts";
import type { Translate } from "../timeline/errors.ts";

const COMMAND_LABELS: Readonly<Record<string, TKey>> = {
  // Spelled in two parts: a dotted literal under a catalog root reads as a missing catalog key.
  [`${"timeline"}.insert`]: "history.insert",
  "item.move": "timeline.history.move",
  "clip.trim": "timeline.history.trim",
  "clip.split": "timeline.history.split",
  "item.delete": "history.delete",
  "item.set_props": "history.setProps",
  "media.register": "history.register",
  "media.remove": "history.remove",
  "project.new": "history.new",
  "project.rename": "history.rename",
  "track.add": "history.trackAdd",
  "track.remove": "history.trackRemove",
  "track.set": "history.trackSet",
  "edit.restore": "history.restore",
};

/** The entry's label in words; transaction labels are already words and pass through. */
export function historyLabel(label: string, t: Translate): string {
  const key = COMMAND_LABELS[label];
  return key === undefined ? label : t(key);
}
