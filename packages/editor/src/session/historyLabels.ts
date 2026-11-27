// SPDX-License-Identifier: GPL-3.0-or-later
// Names for undo and redo entries. The engine logs a transaction under its label ("Move clip",
// set by the timeline gestures) or, for a single command, under the command name
// (`item.delete`). The command names are mapped to the same verbs the menus and the
// design system use ("Delete clip"), so the button reads "Undo Delete clip".

import type { TKey } from "../i18n/index.ts";
import type { Translate } from "../timeline/errors.ts";
import type { UndoStacks } from "./sessionStore.ts";

const COMMAND_LABELS: Readonly<Record<string, TKey>> = {
  // Spelled in two parts: a dotted literal under a catalog root reads as a missing catalog key.
  [`${"timeline"}.insert`]: "history.insert",
  "item.move": "timeline.history.move",
  "clip.trim": "timeline.history.trim",
  "clip.split": "timeline.history.split",
  "item.delete": "history.delete",
  "item.set_props": "history.setProps",
  "item.reset_props": "history.resetProps",
  "item.apply_layout": "history.applyLayout",
  [`${"audio"}.set`]: "history.audioSet",
  [`${"audio"}.set_fade`]: "history.audioFade",
  [`${"audio"}.set_volume`]: "history.audioVolume",
  [`${"audio"}.set_pan`]: "history.audioPan",
  [`${"audio"}.set_gain`]: "history.audioGain",
  [`${"keyframe"}.set`]: "history.keyframeSet",
  [`${"keyframe"}.delete`]: "history.keyframeDelete",
  [`${"keyframe"}.move`]: "history.keyframeMove",
  [`${"keyframe"}.set_interp`]: "history.keyframeEasing",
  [`${"keyframe"}.clear`]: "history.keyframeClear",
  [`${"keyframe"}.paste`]: "history.keyframePaste",
  "media.register": "history.register",
  "media.remove": "history.remove",
  "project.new": "history.new",
  "project.rename": "history.rename",
  "project.set_settings": "history.projectSettings",
  "track.add": "history.trackAdd",
  "track.remove": "history.trackRemove",
  "track.set": "history.trackSet",
  "edit.restore": "history.restore",
  "clip.ripple_trim": "history.rippleTrim",
  "clip.roll": "history.roll",
  "clip.slip": "history.slip",
  "clip.slide": "history.slide",
  "clip.set_speed": "history.setSpeed",
  "clip.reverse": "history.reverse",
  "clip.reset_speed": "history.resetSpeed",
  "clip.freeze_frame": "history.freezeFrame",
  "clip.replace": "history.replaceClip",
  "item.duplicate": "history.duplicate",
  [`${"timeline"}.paste`]: "history.paste",
  [`${"clip"}.paste_attributes`]: "history.pasteAttributes",
  "item.link": "history.link",
  "item.unlink": "history.unlink",
  "item.attach": "history.attach",
  "item.detach": "history.detach",
  [`${"timeline"}.overwrite`]: "history.overwrite",
  [`${"timeline"}.lift`]: "history.lift",
  [`${"timeline"}.extract`]: "history.extract",
  [`${"timeline"}.ripple_delete`]: "history.rippleDelete",
  [`${"timeline"}.delete_gap`]: "history.deleteGap",
  [`${"timeline"}.add_marker`]: "history.markerAdd",
  [`${"timeline"}.move_marker`]: "history.markerMove",
  [`${"timeline"}.remove_marker`]: "history.markerRemove",
  [`${"timeline"}.set_in_out`]: "history.inOutSet",
  [`${"timeline"}.clear_in_out`]: "history.inOutClear",
  "track.reorder": "history.trackReorder",
  "track.set_height": "history.trackHeight",
  "track.set_magnetic": "history.trackMagnetic",
  "media.create_folder": "history.folderCreate",
  "media.rename_folder": "history.folderRename",
  "media.delete_folder": "history.folderDelete",
  "media.move": "history.mediaMove",
  "media.rename": "history.mediaRename",
  "media.add_tags": "history.tagAdd",
  "media.remove_tags": "history.tagRemove",
  "media.rename_tag": "history.tagRename",
  "media.set_rating": "history.rating",
  "media.set_note": "history.note",
  "media.create_collection": "history.collectionCreate",
  "media.rename_collection": "history.collectionRename",
  "media.delete_collection": "history.collectionDelete",
  "media.add_to_collection": "history.collectionAdd",
  "media.remove_from_collection": "history.collectionRemove",
  "media.set_collection_filter": "history.collectionFilter",
  [`${"sequence"}.create`]: "history.sequenceCreate",
  [`${"sequence"}.create_from_media`]: "history.sequenceCreateFromMedia",
  [`${"sequence"}.duplicate`]: "history.sequenceDuplicate",
  [`${"sequence"}.rename`]: "history.sequenceRename",
  [`${"sequence"}.delete`]: "history.sequenceDelete",
  [`${"sequence"}.set_format`]: "history.sequenceFormat",
};

/** Product names an agent label is capitalized to; any other label is shown as it is. */
const KNOWN_AGENTS: Readonly<Record<string, string>> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
  cursor: "Cursor",
};

/**
 * Who made a change, for a label: `agent:<label>` is the label (capitalized for a known product
 * name), `plugin:<id>` the id, `system` "Cutweave". The person (`user`) gets `null`: their own
 * steps carry no name.
 */
export function actorDisplayName(actor: string, t: Translate): string | null {
  if (actor === "user") return null;
  if (actor === "system") return t("history.actorSystem");
  const colon = actor.indexOf(":");
  const kind = colon < 0 ? actor : actor.slice(0, colon);
  const id = colon < 0 ? "" : actor.slice(colon + 1);
  if (kind === "agent") return KNOWN_AGENTS[id.toLowerCase()] ?? id;
  if (kind === "plugin") return id;
  return actor;
}

/** The step in words, with who made it when that was not the person: "Insert clip (Claude)". */
export function stepName(label: string, actor: string | null, t: Translate): string {
  const step = historyLabel(label, t);
  const who = actor === null ? null : actorDisplayName(actor, t);
  return who === null ? step : t("history.stepBy", { step, who });
}

/** The Undo button, menu item and tooltip text: "Undo", "Undo Move clip" or "Undo Insert clip (Claude)". */
export function undoText(stacks: UndoStacks, t: Translate): string {
  if (stacks.nextUndo === null) return t("topbar.undo");
  return t("topbar.undoAction", { label: stepName(stacks.nextUndo, stacks.nextUndoActor, t) });
}

/** The Redo counterpart of {@link undoText}; the named actor is the one who undid the change. */
export function redoText(stacks: UndoStacks, t: Translate): string {
  if (stacks.nextRedo === null) return t("topbar.redo");
  return t("topbar.redoAction", { label: stepName(stacks.nextRedo, stacks.nextRedoActor, t) });
}

/** The entry's label in words; transaction labels are already words and pass through. */
export function historyLabel(label: string, t: Translate): string {
  const key = COMMAND_LABELS[label];
  return key === undefined ? label : t(key);
}
