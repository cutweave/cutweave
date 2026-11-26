// SPDX-License-Identifier: GPL-3.0-or-later
// The application menu model (WP-1M.09): what File, Edit, View and Help hold, which commands are
// enabled, the shortcut of each, and which key press runs which command. Pure and unit-tested;
// `AppMenuBar` draws it with `@cutweave/ui`'s `MenuBar` and `useAppShortcuts` listens for the keys.
//
// It holds what the native menu has (apps/desktop `menu.ts`, kept for macOS, where Settings… sits
// in the app menu), plus Open recent. The shortcuts match Electron's native roles on Windows and Linux:
//
//   File  New project… Ctrl+N · Open project… Ctrl+O · Open recent ▸ · Save Ctrl+S ·
//         Save as… Ctrl+Shift+S · Export… Ctrl+M · Settings… Ctrl+, ·
//         Exit (Windows: the OS's Alt+F4) / Quit Ctrl+Q (Linux)
//   Edit  Undo Ctrl+Z · Redo Ctrl+Y (also Ctrl+Shift+Z) · Cut Ctrl+X · Copy Ctrl+C · Paste Ctrl+V ·
//         Select all Ctrl+A
//   View  Toggle full screen F11 · dev builds: Reload Ctrl+R · Toggle developer tools Ctrl+Shift+I
//   Help  About Cutweave
//
// Keys while typing in a text field: Undo, Redo, Cut, Copy, Paste and Select all belong to the
// field (Chromium handles them; the page never takes Ctrl+C/X/V/A at all). Every other shortcut
// runs from a field too, as the native menu's accelerators did.

import type { TKey } from "../i18n/index.ts";
import { isComposing, keymapModelFor } from "../keymap/store.ts";
import { type KeyCombo, spell, strokeOfEvent } from "../keymap/stroke.ts";

export type AppCommand =
  | "new"
  | "open"
  | "save"
  | "saveAs"
  | "export"
  | "collect"
  | "archive"
  | "quit"
  | "undo"
  | "redo"
  | "cut"
  | "copy"
  | "paste"
  | "selectAll"
  | "toggleFullScreen"
  | "reload"
  | "toggleDevTools"
  | "settings"
  | "about";

/** Where the menu bar is drawn: Windows and Linux differ only in Exit/Quit. */
export type MenuPlatform = "windows" | "linux" | "mac";

export function menuPlatform(platform: string): MenuPlatform {
  if (/^mac|darwin/i.test(platform)) return "mac";
  if (/^win/i.test(platform)) return "windows";
  return "linux";
}

/** A key with modifiers. `mod` is Ctrl (Cmd on macOS). */
export type Chord = KeyCombo;

/** The keymap's id of each menu command (WP-2H.05): its keys come from the keymap, not from here. */
export const COMMAND_IDS: Readonly<Record<AppCommand, string>> = {
  new: "project.new",
  open: "project.open",
  save: "project.save",
  saveAs: "project.save_as",
  export: "export.start",
  collect: "project.collect",
  archive: "project.archive",
  quit: "app.quit",
  undo: "edit.undo",
  redo: "edit.redo",
  cut: "edit.text.cut",
  copy: "edit.text.copy",
  paste: "edit.text.paste",
  selectAll: "edit.text.select_all",
  toggleFullScreen: "window.toggle_full_screen",
  reload: "window.reload",
  toggleDevTools: "window.toggle_dev_tools",
  settings: "app.open_settings",
  about: "app.about",
};

/**
 * The chords of each command, from the keymap (the preset and the person's changes); the first is
 * the one the menu shows. A key sequence of two strokes is not a menu accelerator and is left out.
 */
export function chordsFor(command: AppCommand, platform: MenuPlatform): readonly Chord[] {
  return keymapModelFor(platform)
    .keysOf(COMMAND_IDS[command])
    .filter((combo) => combo.next === undefined);
}

/** How a menu shows a chord: `Ctrl+Shift+S`, `F11` (`⌘` for Ctrl on macOS). */
export function chordLabel(chord: Chord, platform: MenuPlatform): string {
  const parts: string[] = [];
  if (chord.ctrl) parts.push("Ctrl");
  if (chord.mod) parts.push(platform === "mac" ? "⌘" : "Ctrl");
  if (chord.alt) parts.push("Alt");
  if (chord.shift) parts.push("Shift");
  parts.push(chord.key);
  return parts.join("+");
}

/** The shortcut text a menu shows for `command`, if it has one. */
export function shortcutLabel(command: AppCommand, platform: MenuPlatform): string | undefined {
  const first = chordsFor(command, platform)[0];
  return first === undefined ? undefined : chordLabel(first, platform);
}

/**
 * Commands whose keys the page never takes: Chromium already cuts, copies, pastes and selects
 * all on Windows and Linux, in text fields and on the page, exactly as the menu item does.
 */
export const BROWSER_KEYS: ReadonlySet<AppCommand> = new Set(["cut", "copy", "paste", "selectAll"]);

/** Commands a focused text field keeps for itself (its own undo history). */
const FIELD_KEYS: ReadonlySet<AppCommand> = new Set(["undo", "redo"]);

/** Commands whose key repeats while held, as a native accelerator does; the rest act once. */
const REPEATING: ReadonlySet<AppCommand> = new Set(["undo", "redo"]);

/** The slice of a `KeyboardEvent` the shortcuts read. */
export interface ShortcutEvent {
  readonly key: string;
  /** The physical key: a letter still matches when the layout prints another script. */
  readonly code?: string;
  readonly isComposing?: boolean;
  readonly keyCode?: number;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
  readonly repeat?: boolean;
}

/** Commands the page may run from a key (not the browser's own clipboard keys). */
export const KEYED: readonly AppCommand[] = [
  "new",
  "open",
  "save",
  "saveAs",
  "export",
  "settings",
  "quit",
  "undo",
  "redo",
  "toggleFullScreen",
  "reload",
  "toggleDevTools",
];

function matches(chord: Chord, e: ShortcutEvent, platform: MenuPlatform): boolean {
  if (isComposing(e)) return false;
  const stroke = strokeOfEvent(e, platform);
  return stroke !== null && spell(stroke) === spell(chord);
}

/**
 * The command a key press runs, or `null`. `typing`: a text field has the focus (it keeps its
 * own undo and redo). `options.dev` adds Reload and the developer tools.
 */
export function commandForKey(
  e: ShortcutEvent,
  platform: MenuPlatform,
  options: { readonly typing: boolean; readonly dev: boolean },
): AppCommand | null {
  for (const command of KEYED) {
    if ((command === "reload" || command === "toggleDevTools") && !options.dev) continue;
    if (!chordsFor(command, platform).some((chord) => matches(chord, e, platform))) continue;
    if (options.typing && FIELD_KEYS.has(command)) return null;
    if (e.repeat === true && !REPEATING.has(command)) return null;
    return command;
  }
  return null;
}

/** Whether the page leaves `command`'s key to the browser (see `BROWSER_KEYS`). */
export function isBrowserKey(command: AppCommand): boolean {
  return BROWSER_KEYS.has(command);
}

// ---- the menus ---------------------------------------------------------------------------------

/** What the menus depend on. */
export interface AppMenuState {
  readonly platform: MenuPlatform;
  /** Inside the desktop app: quit, full screen and the edit commands are main's to run. */
  readonly desktop: boolean;
  /** An unpackaged (development) desktop build: Reload and developer tools appear. */
  readonly dev: boolean;
  /** The engine is connected, the project known, and no project action is running. */
  readonly live: boolean;
  readonly readOnly: boolean;
  readonly recovering: boolean;
  readonly canUndo: boolean;
  readonly canRedo: boolean;
  /** "Undo Split clip" when the history names the step, else "Undo". */
  readonly undoLabel: string;
  readonly redoLabel: string;
  /** Recent projects, newest first: the label to show and the path to open. */
  readonly recent: readonly { readonly path: string; readonly label: string }[];
  /** File > Export… opens the export dialog (WP-1M.08); off while the timeline is empty. */
  readonly exportAvailable: boolean;
  /** Every registered panel, with whether it is open in the current workspace (View > Panels). */
  readonly panels: readonly {
    readonly id: string;
    readonly label: string;
    readonly checked: boolean;
  }[];
}

export type AppMenuId = "file" | "edit" | "view" | "help";

export type AppMenuEntry =
  | {
      readonly kind: "command";
      readonly command: AppCommand;
      readonly label: string;
      readonly shortcut?: string | undefined;
      readonly enabled: boolean;
    }
  | { readonly kind: "separator"; readonly id: string }
  | {
      /** View > Panels: one checkable row per registered panel (`workspace.toggle_panel.<id>`). */
      readonly kind: "panels";
      readonly label: string;
      readonly enabled: boolean;
      readonly items: AppMenuState["panels"];
    }
  | {
      /** A `workspace.*` command (View > Reset layout, Edit > History). */
      readonly kind: "workspace";
      readonly command: string;
      /** The panel a `workspace.open_panel` entry opens. */
      readonly panel?: string;
      readonly label: string;
      readonly enabled: boolean;
    }
  | {
      readonly kind: "recent";
      readonly label: string;
      readonly enabled: boolean;
      readonly emptyLabel: string;
      readonly items: readonly { readonly path: string; readonly label: string }[];
    };

export interface AppMenu {
  readonly id: AppMenuId;
  readonly label: string;
  readonly items: readonly AppMenuEntry[];
}

/** Translation of a catalog key (`translator(t)`). */
export type MenuTranslate = (key: TKey, vars?: Readonly<Record<string, string>>) => string;

/** File, Edit, View and Help for `state`. */
export function buildAppMenu(state: AppMenuState, t: MenuTranslate): AppMenu[] {
  const { platform } = state;
  const command = (
    cmd: AppCommand,
    label: string,
    enabled: boolean,
  ): Extract<AppMenuEntry, { kind: "command" }> => ({
    kind: "command",
    command: cmd,
    label,
    shortcut: shortcutLabel(cmd, platform),
    enabled,
  });
  const sep = (id: string): AppMenuEntry => ({ kind: "separator", id });
  const { live, readOnly, recovering, desktop } = state;

  const file: AppMenuEntry[] = [
    command("new", t("menu.new"), live),
    command("open", t("menu.open"), live),
    {
      kind: "recent",
      label: t("menu.openRecent"),
      enabled: live,
      emptyLabel: t("topbar.recentEmpty"),
      items: state.recent,
    },
    sep("file-1"),
    command("save", t("menu.save"), live && !readOnly && !recovering),
    command("saveAs", t("menu.saveAs"), live && !recovering),
    sep("file-2"),
    command("export", t("menu.export"), state.exportAvailable && live),
    command("collect", t("menu.collect"), live && !recovering),
    command("archive", t("menu.archive"), live && !recovering),
    sep("file-3"),
    // App preferences (the theme); the engine need not be connected.
    command("settings", t("menu.settings"), true),
    sep("file-4"),
    command("quit", t(platform === "linux" ? "menu.quit" : "menu.exit"), desktop),
  ];
  const edit: AppMenuEntry[] = [
    command("undo", state.undoLabel, live && !readOnly && state.canUndo),
    command("redo", state.redoLabel, live && !readOnly && state.canRedo),
    {
      kind: "workspace",
      command: "workspace.open_panel",
      panel: "history",
      label: t("menu.history"),
      enabled: true,
    },
    sep("edit-1"),
    command("cut", t("menu.cut"), true),
    command("copy", t("menu.copy"), true),
    command("paste", t("menu.paste"), true),
    command("selectAll", t("menu.selectAll"), true),
  ];
  const view: AppMenuEntry[] = [
    { kind: "panels", label: t("menu.panels"), enabled: true, items: state.panels },
    {
      kind: "workspace",
      command: "workspace.reset_layout",
      label: t("workspace.reset"),
      enabled: true,
    },
    {
      kind: "workspace",
      command: "workspace.reset_all",
      label: t("workspace.resetAll"),
      enabled: true,
    },
    sep("view-0"),
    command("toggleFullScreen", t("menu.toggleFullScreen"), desktop),
    ...(desktop && state.dev
      ? [
          sep("view-1"),
          command("reload", t("menu.reload"), true),
          command("toggleDevTools", t("menu.toggleDevTools"), true),
        ]
      : []),
  ];
  const help: AppMenuEntry[] = [command("about", t("menu.about", { name: t("app.name") }), true)];
  return [
    { id: "file", label: t("menu.file"), items: file },
    { id: "edit", label: t("menu.edit"), items: edit },
    { id: "view", label: t("menu.view"), items: view },
    { id: "help", label: t("menu.help"), items: help },
  ];
}

/** Whether `command` is offered and enabled in `menus` (shortcuts follow the menu). */
export function isEnabled(menus: readonly AppMenu[], command: AppCommand): boolean {
  for (const menu of menus) {
    for (const entry of menu.items) {
      if (entry.kind === "command" && entry.command === command) return entry.enabled;
    }
  }
  return false;
}
