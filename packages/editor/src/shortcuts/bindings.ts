// SPDX-License-Identifier: GPL-3.0-or-later
// Every keyboard shortcut the app has, as data (WP-1M.13, WP-2H.05). Settings > Keyboard shortcuts
// and the Settings search list this; nothing here is written by hand twice. The keys are the
// keymap's (`keymap/`): its preset plus the person's changes, which the menu bar, the transport,
// the timeline, the bin and the source view all read, so the page cannot disagree with a handler.
// The commands that have keys but no command of their own in the catalog are the table in
// `keymap/commands.ts`, grouped:
//
//   Menu bar    File, Edit and View commands, the command palette
//   Playback    the transport's keys and the Source view's
//   Timeline    the timeline's own keys
//   Media bin   the bin's keys
//   Dialogs     the few keys every dialog and text field share (the UI library owns them)

import type { TKey } from "../i18n/index.ts";
import { type BindingGroupId, KEY_COMMANDS, type KeyContext } from "../keymap/commands.ts";
import { keymapModelFor } from "../keymap/store.ts";
import { type KeyCombo, strokeParts } from "../keymap/stroke.ts";
import type { MenuPlatform } from "../shell/appMenu.ts";

export type { BindingGroupId, KeyCombo };

/** The groups, in the order the page lists them, with where each works. */
export const BINDING_GROUPS: readonly {
  readonly id: BindingGroupId;
  readonly title: TKey;
}[] = [
  { id: "menu", title: "shortcuts.groups.menu" },
  { id: "playback", title: "shortcuts.groups.playback" },
  { id: "timeline", title: "shortcuts.groups.timeline" },
  { id: "bin", title: "shortcuts.groups.bin" },
  { id: "dialogs", title: "shortcuts.groups.dialogs" },
];

export interface Binding {
  /** The command id: `project.save`, `transport.toggle_play`. Unique. */
  readonly id: string;
  readonly group: BindingGroupId;
  readonly context: KeyContext;
  readonly label: TKey;
  readonly description: TKey;
  /** Alternatives: any of these keys does it. The first is the one a menu shows. */
  readonly combos: readonly KeyCombo[];
  /** Only in an unpackaged (development) build. */
  readonly devOnly?: boolean;
  /** The keys cannot be changed. */
  readonly fixed?: boolean;
}

function toBinding(command: (typeof KEY_COMMANDS)[number], combos: readonly KeyCombo[]): Binding {
  return {
    id: command.id,
    group: command.group,
    context: command.context,
    label: command.label,
    description: command.description,
    combos,
    ...(command.devOnly === true && { devOnly: true }),
    ...(command.fixed === true && { fixed: true }),
  };
}

/**
 * Every shortcut that has a key, grouped in {@link BINDING_GROUPS} order. `dev` adds the ones only a
 * development build has (Reload, developer tools).
 */
export function listBindings(
  platform: MenuPlatform,
  options: { readonly dev?: boolean } = {},
): Binding[] {
  return listCommands(platform, options).filter((b) => b.combos.length > 0);
}

/**
 * Every command a key can run that the keymap's table names, with the keys it has now (none, when
 * the person removed them): the rows of Settings > Keyboard shortcuts.
 */
export function listCommands(
  platform: MenuPlatform,
  options: { readonly dev?: boolean } = {},
): Binding[] {
  const model = keymapModelFor(platform);
  return KEY_COMMANDS.filter((c) => options.dev === true || c.devOnly !== true).map((c) =>
    toBinding(c, model.keysOf(c.id)),
  );
}

// ---- how keys read ----------------------------------------------------------------------------

/** The parts of a combo's first stroke as key caps: `["Ctrl", "Shift", "S"]`, `["⇧", "⌘", "S"]` on macOS. */
export function comboParts(combo: KeyCombo, platform: MenuPlatform): string[] {
  return strokeParts(combo, platform);
}

/** A combo as text: `Ctrl+Shift+S`, a chord `Ctrl+K Ctrl+S`. */
export function comboLabel(combo: KeyCombo, platform: MenuPlatform): string {
  const first = strokeParts(combo, platform).join("+");
  return combo.next === undefined ? first : `${first} ${comboLabel(combo.next, platform)}`;
}

/** The combos of a binding with duplicates removed. */
export function distinctCombos(binding: Binding, platform: MenuPlatform): KeyCombo[] {
  const seen = new Set<string>();
  return binding.combos.filter((combo) => {
    const label = comboLabel(combo, platform);
    if (seen.has(label)) return false;
    seen.add(label);
    return true;
  });
}

/** What search matches besides the name: the keys as written, `Ctrl+S, Ctrl+Shift+S`. */
export function bindingKeysText(binding: Binding, platform: MenuPlatform): string {
  return distinctCombos(binding, platform)
    .map((combo) => comboLabel(combo, platform))
    .join(", ");
}
