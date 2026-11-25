// SPDX-License-Identifier: GPL-3.0-or-later
// The menus of every surface the editor has today (WP-2H.13): one table per surface, each naming
// commands that exist. `registerSurfaceMenus` puts them, the commands' menu text and the shortcut
// hints into the app's services; a work package that adds a command or a panel adds its rows
// here (or through `menuContributions`) and its `CommandInfo` below.
import type { CommandInfo } from "../commands.ts";
import { registerKeyHint } from "../keys.ts";
import type { MenuServices } from "../services.ts";
import type { MenuSurfaceDef } from "../types.ts";
import { BIN_BACKGROUND, BIN_CARD, BIN_CHIP, BIN_FOLDER } from "./bin.ts";
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
  SOURCE_VIEW,
  TRANSPORT_QUALITY,
  TRANSPORT_TIME,
  TRANSPORT_VOLUME,
} from "./preview.ts";
import {
  TIMELINE_CLIP,
  TIMELINE_EDGE,
  TIMELINE_LANE,
  TIMELINE_PLAYHEAD,
  TIMELINE_RULER,
  TIMELINE_TRACK,
} from "./timeline.ts";
import { UI_COMMANDS } from "./uiCommands.ts";

/** Catalog commands the tables name beyond the framework's own (`builtins.ts`). */
export const SURFACE_COMMAND_INFO: readonly CommandInfo[] = [
  {
    name: "item.set_props",
    label: "contextMenu.command.itemSetProps",
    undoable: true,
  },
  { name: "job.cancel", label: "contextMenu.command.jobCancel" },
];

export const SURFACES: readonly MenuSurfaceDef[] = [
  DOCK_TAB,
  DOCK_EMPTY,
  BIN_CARD,
  BIN_FOLDER,
  BIN_BACKGROUND,
  BIN_CHIP,
  PREVIEW_CANVAS,
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
  "chrome.save_now": "project.save",
  "export.open": "export.start",
  "timeline.rename_track": "track.rename",
};

/** Puts the surface tables, UI commands, command texts and hints into `services`. */
export function registerSurfaceMenus(services: MenuServices): void {
  for (const info of SURFACE_COMMAND_INFO) services.commands.register(info);
  // Menu-only: they take their arguments from the menu's target, so the palette does not list them.
  for (const command of UI_COMMANDS) services.ui.register({ ...command, hidden: true });
  for (const surface of SURFACES) services.surfaces.register(surface);
  for (const [command, binding] of Object.entries(KEY_HINTS)) registerKeyHint(command, binding);
}

/** Ids the i18n test must not mistake for catalog keys (they share roots with it). */
export const SURFACE_IDS: readonly string[] = SURFACES.map((s) => s.id);
export const UI_COMMAND_IDS: readonly string[] = UI_COMMANDS.map((c) => c.id);
export const HINT_BINDING_IDS: readonly string[] = Object.values(KEY_HINTS);
