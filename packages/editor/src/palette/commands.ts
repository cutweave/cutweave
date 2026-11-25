// SPDX-License-Identifier: GPL-3.0-or-later
// The UI commands the palette itself adds (WP-2H.04): open a settings page or row, and the
// palette row's own menu (`palette.row`: Run, Copy command id, Assign shortcut…). They go in the
// same tables as every other command, so the keymap (WP-2H.05) binds them too.

import { bindingId } from "../keymap/model.ts";
import type { MenuServices } from "../menus/services.ts";
import type { MenuSurfaceDef, MenuTarget } from "../menus/types.ts";
import { openSettings } from "../settings/settingsStore.ts";
import { closePalette, paletteStore } from "./paletteStore.ts";
import { togglePalette } from "./shortcut.ts";

/** What a palette row's menu acts on (`MenuTarget.data`). */
export interface PaletteRowData {
  /** The row's key in the list. */
  readonly key: string;
  /** The row's command id. */
  readonly command: string;
}

const rowData = (target: MenuTarget): PaletteRowData => target.data as PaletteRowData;

/** The shortcut row of a palette row: its binding id, from its command and fixed arguments. */
export function shortcutRowOf(row: PaletteRowData): string {
  const at = row.key.indexOf("#");
  const args = at < 0 ? undefined : (JSON.parse(row.key.slice(at + 1)) as Record<string, unknown>);
  return `shortcuts.${bindingId(row.command, args)}`;
}

export const PALETTE_ROW_SURFACE: MenuSurfaceDef = {
  id: "palette.row",
  name: (target) => ({ key: "palette.menu.name", vars: { name: target.name ?? "" } }),
  groups: ["run", "command"],
  rows: [
    { command: "palette.run", group: "run", order: 0 },
    { command: "palette.copy_id", group: "command", order: 0 },
    { command: "palette.assign_shortcut", group: "command", order: 10 },
  ],
};

/** Registers the palette's commands and its row surface. */
export function registerPaletteCommands(services: MenuServices): void {
  const ui = services.ui;
  ui.register({
    id: "app.open_settings",
    label: "palette.command.openSettings",
    icon: "settings",
    run: (args) => {
      const { page, row } = (args ?? {}) as { page?: string; row?: string };
      openSettings(page, row);
    },
  });
  // Ctrl/Cmd+K (a keymap binding, WP-2H.05): opens the palette, or closes it when it is open.
  ui.register({
    id: "palette.open",
    label: "palette.title",
    icon: "search",
    hidden: true,
    run: () => togglePalette(),
  });
  ui.register({
    id: "palette.run",
    label: "palette.menu.run",
    icon: "play",
    hidden: true,
    run: (_args, ctx) => paletteStore.getState().requestRun(rowData(ctx.target).key),
  });
  ui.register({
    id: "palette.copy_id",
    label: "palette.menu.copyId",
    icon: "copy",
    hidden: true,
    host: true,
    run: async (_args, ctx) => {
      await navigator.clipboard.writeText(rowData(ctx.target).command);
    },
  });
  ui.register({
    id: "palette.assign_shortcut",
    label: "palette.menu.assignShortcut",
    icon: "keyboard",
    hidden: true,
    run: (_args, ctx) => {
      closePalette();
      // Settings > Keyboard shortcuts, at the command's row.
      openSettings("shortcuts", shortcutRowOf(rowData(ctx.target)));
    },
  });
  services.surfaces.register(PALETTE_ROW_SURFACE);
}
