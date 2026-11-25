// SPDX-License-Identifier: GPL-3.0-or-later
// Every keyboard shortcut the app has, as data (WP-1M.13). Settings > Keyboard shortcuts and the
// Settings search list this; nothing here is written by hand twice. Each group is generated from
// the table the handler itself reads:
//
//   Menu bar    `chordsFor` over `KEYED` and `BROWSER_KEYS` (shell/appMenu.ts), which also decide
//               what a key press runs
//   Playback    the keymap's own bindings (transport/keymap.ts), and the Source view's
//               `SOURCE_KEYS` (preview/sourceKeys.ts)
//   Timeline    `TIMELINE_KEYS` (@cutweave/timeline), which the interaction machine reads
//   Media bin   `BIN_KEYS` (panels/binKeys.ts), which the panel reads
//   Dialogs     the few keys every dialog and text field share (below; the UI library owns them)
//
// A test walks each of those tables, so a handler added without a binding here fails. Read-only
// until the shortcut editor (WP-2H.05) arrives; it will swap the tables, not this list.

import { TIMELINE_KEYS, type TimelineKeyAction } from "@cutweave/timeline";
import type { TKey } from "../i18n/index.ts";
import { PALETTE_COMBO } from "../palette/shortcut.ts";
import { BIN_KEYS, type BinKeyAction } from "../panels/binKeys.ts";
import { SOURCE_KEYS, type SourceKeyAction } from "../preview/sourceKeys.ts";
import {
  type AppCommand,
  BROWSER_KEYS,
  type Chord,
  chordsFor,
  KEYED,
  type MenuPlatform,
} from "../shell/appMenu.ts";
import { type ActionId, keymap } from "../transport/keymap.ts";

export type BindingGroupId = "menu" | "playback" | "timeline" | "bin" | "dialogs";

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

/** One key with its modifiers. `mod` is Ctrl, or Cmd on macOS. */
export interface KeyCombo {
  /** `KeyboardEvent.key`, a letter in either case, or `Space`. */
  readonly key: string;
  readonly mod?: boolean;
  readonly alt?: boolean;
  readonly shift?: boolean;
}

export interface Binding {
  /** Unique across groups: `group.name`. */
  readonly id: string;
  readonly group: BindingGroupId;
  readonly label: TKey;
  readonly description: TKey;
  /** Alternatives: any of these keys does it. */
  readonly combos: readonly KeyCombo[];
  /** Only in an unpackaged (development) build. */
  readonly devOnly?: boolean;
}

// ---- the menu bar -----------------------------------------------------------------------------

const MENU_TEXT: Readonly<Record<AppCommand, { label: TKey; description: TKey }>> = {
  new: { label: "menu.new", description: "shortcuts.menu.new" },
  open: { label: "menu.open", description: "shortcuts.menu.open" },
  save: { label: "menu.save", description: "shortcuts.menu.save" },
  saveAs: { label: "menu.saveAs", description: "shortcuts.menu.saveAs" },
  export: { label: "menu.export", description: "shortcuts.menu.export" },
  settings: { label: "menu.settings", description: "shortcuts.menu.settings" },
  quit: { label: "menu.quit", description: "shortcuts.menu.quit" },
  undo: { label: "shortcuts.menu.undoLabel", description: "shortcuts.menu.undo" },
  redo: { label: "shortcuts.menu.redoLabel", description: "shortcuts.menu.redo" },
  cut: { label: "menu.cut", description: "shortcuts.menu.cut" },
  copy: { label: "menu.copy", description: "shortcuts.menu.copy" },
  paste: { label: "menu.paste", description: "shortcuts.menu.paste" },
  selectAll: { label: "menu.selectAll", description: "shortcuts.menu.selectAll" },
  toggleFullScreen: { label: "menu.toggleFullScreen", description: "shortcuts.menu.fullScreen" },
  reload: { label: "menu.reload", description: "shortcuts.menu.reload" },
  toggleDevTools: { label: "menu.toggleDevTools", description: "shortcuts.menu.devTools" },
  about: { label: "menu.about", description: "shortcuts.menu.quit" },
};

const DEV_COMMANDS: ReadonlySet<AppCommand> = new Set(["reload", "toggleDevTools"]);

const fromChord = (chord: Chord): KeyCombo => ({
  key: chord.key,
  ...(chord.mod === true && { mod: true }),
  ...(chord.shift === true && { shift: true }),
});

function menuBindings(platform: MenuPlatform): Binding[] {
  const out: Binding[] = [];
  // The same commands the key handler runs (`KEYED`) and the clipboard keys Chromium runs.
  for (const command of [...KEYED, ...BROWSER_KEYS]) {
    const chords = chordsFor(command, platform);
    if (chords.length === 0) continue;
    out.push({
      id: `menu.${command}`,
      group: "menu",
      ...MENU_TEXT[command],
      combos: chords.map(fromChord),
      ...(DEV_COMMANDS.has(command) && { devOnly: true }),
    });
  }
  // The command palette (WP-2H.04) listens for its own key, so it is listed beside the menus'.
  out.push({
    id: "menu.commandPalette",
    group: "menu",
    label: "palette.title",
    description: "shortcuts.menu.commandPalette",
    combos: [PALETTE_COMBO],
  });
  return out;
}

// ---- playback ---------------------------------------------------------------------------------

const PLAYBACK_TEXT: Readonly<Record<ActionId, { label: TKey; description: TKey }>> = {
  "transport:togglePlay": {
    label: "shortcuts.playback.togglePlay",
    description: "shortcuts.playback.togglePlayHelp",
  },
  "transport:reverse": {
    label: "shortcuts.playback.reverse",
    description: "shortcuts.playback.reverseHelp",
  },
  "transport:pause": {
    label: "shortcuts.playback.pause",
    description: "shortcuts.playback.pauseHelp",
  },
  "transport:forward": {
    label: "shortcuts.playback.forward",
    description: "shortcuts.playback.forwardHelp",
  },
  "transport:stepBack": {
    label: "shortcuts.playback.stepBack",
    description: "shortcuts.playback.stepBackHelp",
  },
  "transport:stepForward": {
    label: "shortcuts.playback.stepForward",
    description: "shortcuts.playback.stepForwardHelp",
  },
};

/** `Ctrl+Alt+Shift+Meta+<key>` as {@link keymap} spells it, back into a combo. */
export function parseKeymapKey(spelled: string): KeyCombo {
  const parts = spelled.split("+");
  const key = parts.pop() ?? spelled;
  return {
    key,
    ...(parts.some((p) => p === "Ctrl" || p === "Meta") && { mod: true }),
    ...(parts.includes("Alt") && { alt: true }),
    ...(parts.includes("Shift") && { shift: true }),
  };
}

function playbackBindings(): Binding[] {
  const byAction = new Map<ActionId, KeyCombo[]>();
  for (const [spelled, action] of keymap.entries()) {
    byAction.set(action, [...(byAction.get(action) ?? []), parseKeymapKey(spelled)]);
  }
  return [...byAction].map(([action, combos]) => ({
    id: `playback.${action.replace("transport:", "")}`,
    group: "playback",
    ...PLAYBACK_TEXT[action],
    combos,
  }));
}

const SOURCE_TEXT: Readonly<Record<SourceKeyAction, { label: TKey; description: TKey }>> = {
  back: { label: "shortcuts.source.back", description: "shortcuts.source.backHelp" },
  insert: { label: "shortcuts.source.insert", description: "shortcuts.source.insertHelp" },
};

/** The Preview's Source view keys (`SOURCE_KEYS`); Space, J/K/L and the arrows are the keymap's. */
function sourceBindings(): Binding[] {
  return SOURCE_KEYS.map((entry) => ({
    id: `source.${entry.action}`,
    group: "playback",
    ...SOURCE_TEXT[entry.action],
    combos: [{ key: entry.key }],
  }));
}

// ---- the timeline -----------------------------------------------------------------------------

const TIMELINE_TEXT: Readonly<Record<TimelineKeyAction, { label: TKey; description: TKey }>> = {
  cancel: { label: "shortcuts.timeline.cancel", description: "shortcuts.timeline.cancelHelp" },
  split: { label: "shortcuts.timeline.split", description: "shortcuts.timeline.splitHelp" },
  delete: { label: "shortcuts.timeline.delete", description: "shortcuts.timeline.deleteHelp" },
  "toggle-snapping": {
    label: "shortcuts.timeline.snapping",
    description: "shortcuts.timeline.snappingHelp",
  },
  "rename-track": {
    label: "shortcuts.timeline.renameTrack",
    description: "shortcuts.timeline.renameTrackHelp",
  },
};

function timelineBindings(): Binding[] {
  return TIMELINE_KEYS.map((entry) => ({
    id: `timeline.${entry.action}`,
    group: "timeline",
    ...TIMELINE_TEXT[entry.action],
    combos: entry.keys.map((key) => ({ key })),
  }));
}

// ---- the media bin ----------------------------------------------------------------------------

const BIN_TEXT: Readonly<Record<BinKeyAction, { label: TKey; description: TKey; id: string }>> = {
  moveNext: { id: "move", label: "shortcuts.bin.move", description: "shortcuts.bin.moveHelp" },
  movePrevious: { id: "move", label: "shortcuts.bin.move", description: "shortcuts.bin.moveHelp" },
  moveDown: { id: "move", label: "shortcuts.bin.move", description: "shortcuts.bin.moveHelp" },
  moveUp: { id: "move", label: "shortcuts.bin.move", description: "shortcuts.bin.moveHelp" },
  first: { id: "edges", label: "shortcuts.bin.edges", description: "shortcuts.bin.edgesHelp" },
  last: { id: "edges", label: "shortcuts.bin.edges", description: "shortcuts.bin.edgesHelp" },
  insert: { id: "insert", label: "shortcuts.bin.insert", description: "shortcuts.bin.insertHelp" },
  play: { id: "play", label: "shortcuts.bin.play", description: "shortcuts.bin.playHelp" },
  remove: { id: "remove", label: "shortcuts.bin.remove", description: "shortcuts.bin.removeHelp" },
  import: { id: "import", label: "shortcuts.bin.import", description: "shortcuts.bin.importHelp" },
  search: { id: "search", label: "shortcuts.bin.search", description: "shortcuts.bin.searchHelp" },
  up: { id: "up", label: "shortcuts.bin.up", description: "shortcuts.bin.upHelp" },
  rename: { id: "rename", label: "shortcuts.bin.rename", description: "shortcuts.bin.renameHelp" },
  selectAll: {
    id: "select-all",
    label: "shortcuts.bin.selectAll",
    description: "shortcuts.bin.selectAllHelp",
  },
  newFolder: {
    id: "new-folder",
    label: "shortcuts.bin.newFolder",
    description: "shortcuts.bin.newFolderHelp",
  },
};

function binBindings(): Binding[] {
  const out = new Map<string, { binding: Binding; combos: KeyCombo[] }>();
  for (const entry of BIN_KEYS) {
    const text = BIN_TEXT[entry.action];
    const combo: KeyCombo = {
      key: entry.key,
      ...(entry.mod === true && { mod: true }),
      ...(entry.alt === true && { alt: true }),
      ...(entry.shift === true && { shift: true }),
    };
    const known = out.get(text.id);
    if (known !== undefined) known.combos.push(combo);
    else {
      const combos = [combo];
      out.set(text.id, {
        combos,
        binding: {
          id: `bin.${text.id}`,
          group: "bin",
          label: text.label,
          description: text.description,
          combos,
        },
      });
    }
  }
  return [...out.values()].map((v) => v.binding);
}

// ---- dialogs and text fields ------------------------------------------------------------------

/** The keys every dialog and text field shares. The UI library handles them, so they are listed here. */
const DIALOG_BINDINGS: readonly Binding[] = [
  {
    id: "dialogs.close",
    group: "dialogs",
    label: "shortcuts.dialogs.close",
    description: "shortcuts.dialogs.closeHelp",
    combos: [{ key: "Escape" }],
  },
  {
    id: "dialogs.confirm",
    group: "dialogs",
    label: "shortcuts.dialogs.confirm",
    description: "shortcuts.dialogs.confirmHelp",
    combos: [{ key: "Enter" }],
  },
  {
    id: "dialogs.next",
    group: "dialogs",
    label: "shortcuts.dialogs.next",
    description: "shortcuts.dialogs.nextHelp",
    combos: [{ key: "Tab" }, { key: "Tab", shift: true }],
  },
];

// ---- the list ---------------------------------------------------------------------------------

/**
 * Every shortcut, grouped in {@link BINDING_GROUPS} order. `dev` adds the ones only a
 * development build has (Reload, developer tools).
 */
export function listBindings(
  platform: MenuPlatform,
  options: { readonly dev?: boolean } = {},
): Binding[] {
  const all = [
    ...menuBindings(platform),
    ...playbackBindings(),
    ...sourceBindings(),
    ...timelineBindings(),
    ...binBindings(),
    ...DIALOG_BINDINGS,
  ];
  return all.filter((b) => options.dev === true || b.devOnly !== true);
}

// ---- how keys read ----------------------------------------------------------------------------

const KEY_NAMES: Readonly<Record<string, string>> = {
  " ": "Space",
  ArrowLeft: "←",
  ArrowRight: "→",
  ArrowUp: "↑",
  ArrowDown: "↓",
  Escape: "Esc",
};

/** The parts of a combo as key caps: `["Ctrl", "Shift", "S"]`, or `["⇧", "⌘", "S"]` on macOS. */
export function comboParts(combo: KeyCombo, platform: MenuPlatform): string[] {
  const mac = platform === "mac";
  const key =
    KEY_NAMES[combo.key] ?? (combo.key.length === 1 ? combo.key.toUpperCase() : combo.key);
  return [
    ...(combo.mod === true ? [mac ? "⌘" : "Ctrl"] : []),
    ...(combo.alt === true ? [mac ? "⌥" : "Alt"] : []),
    ...(combo.shift === true ? [mac ? "⇧" : "Shift"] : []),
    key,
  ];
}

/** A combo as text: `Ctrl+Shift+S`. */
export function comboLabel(combo: KeyCombo, platform: MenuPlatform): string {
  return comboParts(combo, platform).join("+");
}

/**
 * The combos of a binding with the duplicates a letter's two cases would make removed (the
 * timeline table lists `s`, the page shows `S`).
 */
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
