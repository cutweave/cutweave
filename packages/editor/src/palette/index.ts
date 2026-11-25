// SPDX-License-Identifier: GPL-3.0-or-later
// The command palette (WP-2H.04). Importing this registers the palette's commands and the
// editor-side behaviour of a few registry commands (see `builtinActions.ts`) on the app's tables.
//
//   openPalette()                    open it (Ctrl/Cmd+K does the same)
//   registerPaletteAction(cmd, fn)   carry out a registry command the editor's own way
//   commandCatalog.getState().register(c)   list a command that is not in schema/commands.json
//   registerCommandInfo / registerUiCommand (menus)  give a command a label, an icon, a `when`
import { menuServices } from "../menus/defaults.ts";
import { registerBuiltinPaletteActions } from "./builtinActions.ts";
import { registerPaletteCommands } from "./commands.ts";

registerPaletteCommands(menuServices);
registerBuiltinPaletteActions();

export { registerPaletteAction } from "./actions.ts";
export { CommandPalette } from "./CommandPalette.tsx";
export { type CatalogCommand, commandCatalog } from "./catalog.ts";
export { buildEntries, type PaletteEntry, type PaletteEnv } from "./entries.ts";
export { appPaletteEnv } from "./env.ts";
export { closePalette, openPalette, paletteStore, usePaletteOpen } from "./paletteStore.ts";
export { togglePalette } from "./shortcut.ts";
