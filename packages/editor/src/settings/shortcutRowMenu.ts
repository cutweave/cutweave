// SPDX-License-Identifier: GPL-3.0-or-later
// The shortcut editor's row menu (`shortcuts.row`, WP-2H.05, research 09): Reset binding, Remove
// binding and Copy command id. They are UI commands like the palette row's, so the menu, the
// palette and the keymap name them the same way. What they change is the person's own keys
// (`shortcuts.bindings`), never the project.
import type { MenuServices } from "../menus/services.ts";
import type { MenuContext, MenuSurfaceDef, MenuTarget } from "../menus/types.ts";

/** What a shortcut row's menu acts on (`MenuTarget.data`). */
export interface ShortcutRowData {
  /** The binding id (a command, or `command:argument`). */
  readonly id: string;
  /** The keys differ from the preset's. */
  readonly changed: boolean;
  /** The command has at least one key now. */
  readonly hasKeys: boolean;
}

const rowData = (target: MenuTarget): ShortcutRowData => target.data as ShortcutRowData;

export const SHORTCUT_ROW_SURFACE: MenuSurfaceDef = {
  id: "shortcuts.row",
  name: (target) => ({ key: "palette.menu.name", vars: { name: target.name ?? "" } }),
  groups: ["edit", "command"],
  rows: [
    {
      command: "shortcuts.reset_binding",
      group: "edit",
      order: 0,
      enabledIf: (ctx: MenuContext) => rowData(ctx.target).changed,
      reason: "keymap.row.notChanged",
    },
    {
      command: "shortcuts.remove_binding",
      group: "edit",
      order: 10,
      enabledIf: (ctx: MenuContext) => rowData(ctx.target).hasKeys,
      reason: "keymap.row.noKeys",
    },
    { command: "shortcuts.copy_id", group: "command", order: 0 },
  ],
};

export interface ShortcutRowActions {
  reset(id: string): void;
  remove(id: string): void;
}

/** Registers the row menu's commands and surface. */
export function registerShortcutRowMenu(services: MenuServices, actions: ShortcutRowActions): void {
  const ui = services.ui;
  ui.register({
    id: "shortcuts.reset_binding",
    label: "keymap.row.reset",
    icon: "undo-2",
    hidden: true,
    run: (_args, ctx) => actions.reset(rowData(ctx.target).id),
  });
  ui.register({
    id: "shortcuts.remove_binding",
    label: "keymap.row.remove",
    icon: "x",
    hidden: true,
    run: (_args, ctx) => actions.remove(rowData(ctx.target).id),
  });
  ui.register({
    id: "shortcuts.copy_id",
    label: "palette.menu.copyId",
    icon: "copy",
    hidden: true,
    host: true,
    run: async (_args, ctx) => {
      await navigator.clipboard.writeText(rowData(ctx.target).id);
    },
  });
  services.surfaces.register(SHORTCUT_ROW_SURFACE);
}
