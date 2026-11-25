// SPDX-License-Identifier: GPL-3.0-or-later
// Shortcut hints for menu rows and palette rows (WP-2H.12, WP-2H.05): `keysFor(command)` is the
// first key the keymap gives the command now, so a rebinding in Settings shows in the menus at
// once, with no restart. A row's command is the binding id; the few rows whose command is named
// differently from the binding register an alias.

import { comboText, keymapModelFor } from "../keymap/store.ts";
import type { MenuPlatform } from "../shell/appMenu.ts";

/** Commands the menus show a key for (the others find theirs by the same name when they have one). */
const HINTED: readonly string[] = [
  "clip.split",
  "item.delete",
  "edit.undo",
  "edit.redo",
  "media.remove",
  "edit.text.cut",
  "edit.text.copy",
  "edit.text.paste",
  "edit.text.select_all",
];

/** Command (catalog or UI) to the binding that runs it from the keyboard, when the ids differ. */
const ALIAS: Record<string, string> = {};

/** A feature WP whose command is bound under another binding's id says so here. */
export function registerKeyHint(command: string, bindingId: string): void {
  ALIAS[command] = bindingId;
}

/** The first key of `command` as text (`Ctrl+Z`), or `undefined` when it has none. */
export function keysFor(command: string, platform: MenuPlatform): string | undefined {
  const first = keymapModelFor(platform).keysOf(ALIAS[command] ?? command)[0];
  return first === undefined ? undefined : comboText(first, platform);
}

/** Every command that has a hint, for the walk test. */
export function hintedCommands(): readonly string[] {
  return [...HINTED, ...Object.keys(ALIAS)];
}

/** The binding ids the hints point at, for the walk test. */
export function hintBindingIds(): readonly string[] {
  return [...HINTED, ...Object.values(ALIAS)];
}
