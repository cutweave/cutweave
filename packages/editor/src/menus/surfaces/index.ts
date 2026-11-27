// SPDX-License-Identifier: GPL-3.0-or-later
// The menus of every surface the editor has today (WP-2H.13): one table per surface, each naming
// commands that exist. `registerSurfaceMenus` puts them, the commands' menu text and the shortcut
// hints into the app's services; a work package that adds a command or a panel adds its rows
// here (or through `menuContributions`) and its `CommandInfo` below.
import type { CommandInfo } from "../commands.ts";
import { registerKeyHint } from "../keys.ts";
import type { MenuServices } from "../services.ts";
import type { MenuSurfaceDef } from "../types.ts";
import { BIN_BACKGROUND, BIN_CARD, BIN_CHIP, BIN_COLLECTION, BIN_FOLDER } from "./bin.ts";
import {
  CHROME_ENGINE,
  CHROME_EXPORT,
  CHROME_HISTORY,
  CHROME_SAVE,
  CHROME_TOAST,
  CHROME_WORKSPACE,
} from "./chrome.ts";
import { DOCK_EMPTY, DOCK_TAB } from "./dock.ts";
import {
  INSPECTOR_PATH,
  INSPECTOR_PICTURE,
  INSPECTOR_ROW,
  INSPECTOR_SECTION,
  SETTINGS_ROW,
} from "./inspector.ts";
import {
  PREVIEW_CANVAS,
  PREVIEW_ITEM,
  SOURCE_VIEW,
  TRANSPORT_QUALITY,
  TRANSPORT_TIME,
  TRANSPORT_VOLUME,
} from "./preview.ts";
import {
  TIMELINE_CLIP,
  TIMELINE_EDGE,
  TIMELINE_LANE,
  TIMELINE_MARKER,
  TIMELINE_PLAYHEAD,
  TIMELINE_RULER,
  TIMELINE_TRACK,
} from "./timeline.ts";
import { UI_COMMANDS } from "./uiCommands.ts";

/** Catalog commands the tables name beyond the framework's own (`builtins.ts`). */
export const SURFACE_COMMAND_INFO: readonly CommandInfo[] = [
  // The palette's rows for the two WP-2F.05d commands: the menus' words, and the editor carries
  // them out its own way (`palette/builtinActions.ts`), reading the frame on screen and asking
  // for the pictures.
  {
    name: "media.import_frame",
    label: "contextMenu.command.mediaImportFrame",
    icon: "image",
    when: "engine.live",
  },
  {
    name: "media.import_images_as_slideshow",
    label: "contextMenu.command.mediaImportImagesAsSlideshow",
    icon: "film",
    when: "engine.live",
  },
  {
    name: "item.set_props",
    label: "contextMenu.command.itemSetProps",
    undoable: true,
  },
  { name: "job.cancel", label: "contextMenu.command.jobCancel" },
  // Proxy ▸ Auto / Always / Never on a bin card (#215): one undo step for the assets chosen.
  {
    name: "media.set_proxy_mode",
    label: "contextMenu.command.mediaSetProxyMode",
    undoable: true,
  },
  // Set rating ▸ on a bin card and Add tag… (WP-2F.06b): one undo step for the assets chosen.
  // The palette opens the stars and the tag editor for the selected cards
  // (`palette/builtinActions.ts`).
  {
    name: "media.set_rating",
    label: "contextMenu.command.mediaSetRating",
    icon: "star",
    undoable: true,
    when: "engine.live",
  },
  {
    name: "media.add_tags",
    label: "contextMenu.command.mediaAddTags",
    undoable: true,
    when: "engine.live",
  },
  // Collections (WP-2F.06c). The palette asks for a name, and lists the manual collections for
  // the selected cards (`palette/builtinActions.ts`); a card's menu has Remove from collection
  // while a manual collection is open.
  {
    name: "media.create_collection",
    label: "contextMenu.command.mediaCreateCollection",
    icon: "plus",
    undoable: true,
    when: "engine.live",
  },
  {
    name: "media.add_to_collection",
    label: "contextMenu.command.mediaAddToCollection",
    undoable: true,
    when: "engine.live",
  },
  {
    name: "media.remove_from_collection",
    label: "contextMenu.command.mediaRemoveFromCollection",
    undoable: true,
  },
  {
    name: "item.duplicate",
    label: "contextMenu.command.itemDuplicate",
    labelMany: "contextMenu.command.itemDuplicateMany",
    icon: "copy",
    undoable: true,
  },
  // WP-2A.10: the timeline's edits from WP-2A.02 and WP-2A.03.
  {
    name: "timeline.ripple_delete",
    label: "contextMenu.command.timelineRippleDelete",
    labelMany: "contextMenu.command.timelineRippleDeleteMany",
    icon: "trash-2",
    destructive: true,
    undoable: true,
  },
  {
    name: "timeline.add_marker",
    label: "contextMenu.command.timelineAddMarker",
    icon: "bookmark",
    undoable: true,
  },
  // Issue #222: the marker's Convert to chapter, and the clip's Color label.
  {
    name: "timeline.update_marker",
    label: "contextMenu.command.timelineUpdateMarker",
    undoable: true,
  },
  {
    name: "item.set_label_color",
    label: "contextMenu.command.itemSetLabelColor",
    undoable: true,
  },
  {
    name: "timeline.remove_marker",
    label: "contextMenu.command.timelineRemoveMarker",
    icon: "trash-2",
    destructive: true,
    undoable: true,
  },
  { name: "timeline.set_in_out", label: "contextMenu.command.timelineSetInOut", undoable: true },
  {
    name: "timeline.clear_in_out",
    label: "contextMenu.command.timelineClearInOut",
    undoable: true,
  },
  { name: "timeline.delete_gap", label: "contextMenu.command.timelineDeleteGap", undoable: true },
  {
    name: "timeline.overwrite",
    label: "contextMenu.command.timelineOverwrite",
    destructive: true,
    undoable: true,
  },
  {
    name: "timeline.lift",
    label: "contextMenu.command.timelineLift",
    destructive: true,
    undoable: true,
  },
  {
    name: "timeline.extract",
    label: "contextMenu.command.timelineExtract",
    destructive: true,
    undoable: true,
  },
  {
    name: "track.set_magnetic",
    label: "contextMenu.command.trackSetMagnetic",
    icon: "magnet",
    undoable: true,
  },
  { name: "track.set_height", label: "contextMenu.command.trackSetHeight", undoable: true },
  { name: "track.reorder", label: "contextMenu.command.trackReorder", undoable: true },
];

export const SURFACES: readonly MenuSurfaceDef[] = [
  DOCK_TAB,
  DOCK_EMPTY,
  BIN_CARD,
  BIN_FOLDER,
  BIN_COLLECTION,
  BIN_BACKGROUND,
  BIN_CHIP,
  PREVIEW_CANVAS,
  PREVIEW_ITEM,
  SOURCE_VIEW,
  TRANSPORT_TIME,
  TRANSPORT_VOLUME,
  TRANSPORT_QUALITY,
  TIMELINE_CLIP,
  TIMELINE_EDGE,
  TIMELINE_LANE,
  TIMELINE_TRACK,
  TIMELINE_RULER,
  TIMELINE_PLAYHEAD,
  TIMELINE_MARKER,
  INSPECTOR_SECTION,
  INSPECTOR_ROW,
  INSPECTOR_PATH,
  INSPECTOR_PICTURE,
  SETTINGS_ROW,
  CHROME_SAVE,
  CHROME_EXPORT,
  CHROME_WORKSPACE,
  CHROME_HISTORY,
  CHROME_TOAST,
  CHROME_ENGINE,
];

/** The shortcut each command is bound to today (the hint reads the keymap, never a literal). */
const KEY_HINTS: Readonly<Record<string, string>> = {
  "timeline.insert": "bin.insert",
  "bin.import": "bin.import",
  "bin.import_folder": "bin.import_folder",
  "bin.play_in_source": "bin.play",
  "bin.remove_media": "media.remove",
  "bin.remove_selected": "media.remove",
  "bin.new_folder": "bin.new_folder",
  "bin.rename": "bin.rename",
  "bin.select_all": "edit.text.select_all",
  "bin.open_folder": "bin.insert",
  "bin.delete_folders": "media.remove",
  "transport.toggle_play": "transport.toggle_play",
  "source.back": "source.back",
  "source.insert": "source.insert",
  "source.overwrite": "source.overwrite",
  "source.mark_in": "source.mark_in",
  "source.mark_out": "source.mark_out",
  "source.clear_marks": "source.clear_marks",
  "timeline.match_frame": "timeline.match_frame",
  "chrome.save_now": "project.save",
  "export.open": "export.start",
  "timeline.rename_track": "track.rename",
  "timeline.cut": "edit.text.cut",
  "timeline.copy_clips": "edit.text.copy",
  "timeline.paste_here": "edit.text.paste",
  "timeline.split_at_playhead": "clip.split",
};

/**
 * UI commands the palette lists too: they read the editor's own state, not a menu's target. Save
 * as smart collection… stores the bin's search and filters (WP-2F.06c).
 */
const PALETTE_UI_COMMANDS: ReadonlySet<string> = new Set(["bin.save_smart_collection"]);

/** Puts the surface tables, UI commands, command texts and hints into `services`. */
export function registerSurfaceMenus(services: MenuServices): void {
  for (const info of SURFACE_COMMAND_INFO) services.commands.register(info);
  // Menu-only: they take their arguments from the menu's target, so the palette does not list
  // them, except the few that need none (`PALETTE_UI_COMMANDS`).
  for (const command of UI_COMMANDS) {
    services.ui.register({ ...command, hidden: !PALETTE_UI_COMMANDS.has(command.id) });
  }
  for (const surface of SURFACES) services.surfaces.register(surface);
  for (const [command, binding] of Object.entries(KEY_HINTS)) registerKeyHint(command, binding);
}

/** Ids the i18n test must not mistake for catalog keys (they share roots with it). */
export const SURFACE_IDS: readonly string[] = SURFACES.map((s) => s.id);
export const UI_COMMAND_IDS: readonly string[] = UI_COMMANDS.map((c) => c.id);
export const HINT_BINDING_IDS: readonly string[] = Object.values(KEY_HINTS);
