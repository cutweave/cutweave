// SPDX-License-Identifier: GPL-3.0-or-later
// What the framework itself registers (WP-2H.12): how the registry commands the first menus use
// read in a menu, the text-field menu, and the development-only "Inspect element". Surfaces for
// the rest of the editor are WP-2H.13's, each in its own table under `menus/surfaces/`.
import type { CommandInfo } from "./commands.ts";
import type { MenuServices } from "./services.ts";
import { FIELD_COMMANDS, FIELD_SURFACE, INSPECT_COMMAND, INSPECT_SURFACE } from "./textField.ts";

/**
 * Registry commands with their menu text. `destructive` and `undoable` copy the catalog
 * (`schema/commands.json`); the walk test fails when they drift. Add a row here when a surface
 * table first names a command.
 */
export const COMMAND_INFO: readonly CommandInfo[] = [
  { name: "clip.split", label: "contextMenu.command.clip_split", icon: "scissors", undoable: true },
  {
    name: "item.delete",
    label: "contextMenu.command.item_delete",
    icon: "trash-2",
    destructive: true,
    undoable: true,
  },
  { name: "timeline.insert", label: "contextMenu.command.timeline_insert", undoable: true },
  { name: "track.add", label: "contextMenu.command.track_add", icon: "plus", undoable: true },
  {
    name: "track.remove",
    label: "contextMenu.command.track_remove",
    icon: "trash-2",
    destructive: true,
    undoable: true,
  },
  { name: "track.set", label: "contextMenu.command.track_set", undoable: true },
  {
    name: "media.remove",
    label: "contextMenu.command.media_remove",
    icon: "trash-2",
    destructive: true,
    undoable: true,
  },
  { name: "edit.undo", label: "contextMenu.command.edit_undo", icon: "undo-2" },
  { name: "edit.redo", label: "contextMenu.command.edit_redo", icon: "redo-2" },
  { name: "view.select", label: "contextMenu.command.view_select" },
  { name: "view.set_playhead", label: "contextMenu.command.view_set_playhead" },
];

/** Puts the framework's own commands and surfaces into `services`. */
export function registerBuiltinMenus(services: MenuServices): void {
  for (const info of COMMAND_INFO) services.commands.register(info);
  for (const command of [...FIELD_COMMANDS, INSPECT_COMMAND]) services.ui.register(command);
  services.surfaces.register(FIELD_SURFACE);
  services.surfaces.register(INSPECT_SURFACE);
}
