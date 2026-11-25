// SPDX-License-Identifier: GPL-3.0-or-later
// Shortcut hints for menu rows (WP-2H.12): `keysFor(command)` reads today's binding tables
// (`shortcuts/bindings.ts`) through a map from command to binding. When the keymap engine
// (WP-2H.05) lands it replaces this body, not its callers.
import { type Binding, comboLabel, distinctCombos, listBindings } from "../shortcuts/bindings.ts";
import type { MenuPlatform } from "../shell/appMenu.ts";

/** Command (catalog or UI) to the binding that runs it from the keyboard. */
const BINDING_OF: Record<string, string> = {
  "clip.split": "timeline.split",
  "item.delete": "timeline.delete",
  "edit.undo": "menu.undo",
  "edit.redo": "menu.redo",
  "media.remove": "bin.remove",
  "edit.text.cut": "menu.cut",
  "edit.text.copy": "menu.copy",
  "edit.text.paste": "menu.paste",
  "edit.text.select_all": "menu.selectAll",
};

/** A feature WP that adds a command and its key says so here. */
export function registerKeyHint(command: string, bindingId: string): void {
  BINDING_OF[command] = bindingId;
}

const byPlatform = new Map<MenuPlatform, ReadonlyMap<string, Binding>>();

function bindings(platform: MenuPlatform): ReadonlyMap<string, Binding> {
  let found = byPlatform.get(platform);
  if (found === undefined) {
    found = new Map(listBindings(platform).map((b) => [b.id, b]));
    byPlatform.set(platform, found);
  }
  return found;
}

/** The first key of `command` as text (`Ctrl+Z`), or `undefined` when it has none. */
export function keysFor(command: string, platform: MenuPlatform): string | undefined {
  const id = BINDING_OF[command];
  if (id === undefined) return undefined;
  const binding = bindings(platform).get(id);
  const first = binding === undefined ? undefined : distinctCombos(binding, platform)[0];
  return first === undefined ? undefined : comboLabel(first, platform);
}

/** Every command that has a hint, for the walk test. */
export function hintedCommands(): readonly string[] {
  return Object.keys(BINDING_OF);
}

/** The binding ids the hints point at, for the walk test. */
export function hintBindingIds(): readonly string[] {
  return Object.values(BINDING_OF);
}
