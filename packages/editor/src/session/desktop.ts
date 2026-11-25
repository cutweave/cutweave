// SPDX-License-Identifier: GPL-3.0-or-later
// The part of `window.cutweave` (apps/desktop `shared/api.ts`) the project flows and the title
// bar use: native folder picking, the recent-projects list that main keeps, the native menu
// commands (macOS), the window-close handshake (main asks, the editor answers once unsaved work
// is settled), and the actions of the in-app menu bar only main can take (WP-1M.09: quit, full
// screen, the edit commands, About, the theme of the OS window controls, and the dev-only reload
// and developer tools). `null` where there is no desktop shell (plain `vite` dev, Vitest): the
// flows then have no native dialogs and the page handles every shortcut itself.

import type { PreferenceChanges, PreferenceMap, SkippedSetting } from "@cutweave/preferences";

/** "Export settings…": where main wrote the file, or that the person cancelled the picker. */
export type PrefsExportResult =
  | { readonly status: "saved"; readonly path: string; readonly count: number }
  | { readonly status: "cancelled" };

/** "Import settings…": what was applied and what was skipped, or why nothing was. */
export type PrefsImportResult =
  | {
      readonly status: "imported";
      readonly applied: readonly string[];
      readonly skipped: readonly SkippedSetting[];
    }
  | { readonly status: "invalid"; readonly reason: "unreadable" | "not-settings" | "newer" }
  | { readonly status: "cancelled" };

/** The preferences store main keeps (`window.cutweave.prefs`, WP-1M.11). */
export interface PrefsBridge {
  get(): Promise<{ readonly values: PreferenceMap; readonly defaults: PreferenceMap }>;
  set(key: string, value: unknown): Promise<void>;
  reset(keys: readonly string[]): Promise<void>;
  exportToFile(only?: "shortcuts"): Promise<PrefsExportResult>;
  importFromFile(only?: "shortcuts"): Promise<PrefsImportResult>;
  onChanged(listener: (changes: PreferenceChanges) => void): () => void;
}

/** The agent hosts Settings > Agents and MCP has a setup for (WP-1J.05). */
export const AGENT_CLIENTS = ["claude-code", "claude-desktop", "cursor", "codex", "stdio"] as const;
export type AgentClient = (typeof AGENT_CLIENTS)[number];

/** How agents connect to this window: the bundled CLI (null when this build has none). */
export interface AgentConnectInfo {
  readonly cliPath: string | null;
  readonly setups: readonly { readonly client: AgentClient; readonly text: string }[];
}

/** "Add folder…": what happened to the folder the person picked in main's folder picker. */
export type AgentFolderResult =
  | { readonly status: "added" | "exists"; readonly folder: string }
  | { readonly status: "full" | "invalid" | "cancelled" };

/**
 * Settings > Agents and MCP (`window.cutweave.agents`, WP-1J.05). Main owns the folder list:
 * the page never names a folder agents may use, it only asks main to pick one or to remove a
 * stored one. Main builds the client setups and writes the clipboard.
 */
export interface AgentsBridge {
  addFolder(): Promise<AgentFolderResult>;
  removeFolder(folder: string): Promise<void>;
  connectInfo(): Promise<AgentConnectInfo>;
  copySetup(client: AgentClient): Promise<string>;
}

/** Menu commands main sends (a subset of the desktop `MenuCommand`). */
export type SessionMenuCommand = "new" | "open" | "save" | "saveAs" | "undo" | "redo";

/** Help > About: the app and runtime versions main reports. */
export interface AboutInfo {
  readonly name: string;
  readonly version: string;
  readonly electron: string;
  readonly chrome: string;
  readonly node: string;
  readonly v8: string;
  readonly platform: string;
}

/** Edit commands main runs on the focused element (the native menu's edit roles). */
export type EditAction = "cut" | "copy" | "paste" | "selectAll";

export interface DesktopBridge {
  readonly app?: {
    getInfo(): Promise<{ readonly platform?: string; readonly dev?: true }>;
    about(): Promise<AboutInfo>;
    /** Quits; the window-close handshake settles unsaved work first. */
    quit(): Promise<void>;
  };
  readonly dialog: {
    /** Native folder picker; `null` when cancelled. */
    openProjectFolder(): Promise<string | null>;
    /**
     * Native "save as" picker for the exported video (WP-1M.08): starts at `defaultPath`
     * (a full path or just a file name); resolves to the chosen `.mp4` path, `null` when cancelled.
     */
    chooseExportFile?(defaultPath: string): Promise<string | null>;
  };
  /** Reveals a file in the OS file manager (WP-1M.08: "Reveal in folder" after an export). */
  readonly shell?: {
    showItemInFolder(path: string): Promise<void>;
    /** "Open settings folder" and "Open logs folder" (WP-1M.11). */
    openAppFolder?(which: "settings" | "logs"): Promise<void>;
  };
  /** App preferences, kept by main in `preferences.json` (WP-1M.11). */
  readonly prefs?: PrefsBridge;
  /** Settings > Agents and MCP (WP-1J.05). */
  readonly agents?: AgentsBridge;
  readonly edit?: {
    run(action: EditAction): Promise<void>;
  };
  readonly menu?: {
    onCommand(listener: (command: string) => void): () => void;
  };
  readonly recent?: {
    list(): Promise<readonly string[]>;
    add(path: string): Promise<void>;
    remove(path: string): Promise<void>;
    /** Forgets every recent project (Settings > General). */
    clear?(): Promise<void>;
  };
  readonly window?: {
    /** Tells main whether closing needs the editor's say (unsaved changes). */
    setDirty(dirty: boolean): Promise<void>;
    /** Main wants to close the window and the project is dirty: settle it, then `confirmClose`. */
    onCloseRequested(listener: () => void): () => void;
    /** Closes the window without asking again. */
    confirmClose(): Promise<void>;
    toggleFullScreen?(): Promise<void>;
    /** The theme the page shows; the OS window controls (Windows, Linux) take its colors. */
    setTheme?(theme: "dark" | "light"): Promise<void>;
    /** Development builds only (main refuses otherwise). */
    reload?(): Promise<void>;
    /** Development builds only (main refuses otherwise). */
    toggleDevTools?(): Promise<void>;
    /** Development builds only: "Inspect element" at a point (main refuses otherwise). */
    inspectElement?(x: number, y: number): Promise<void>;
  };
}

/**
 * Whether the page runs on macOS. There the desktop app keeps the native app menu (which sends
 * its commands as `menu.onCommand`) and the traffic lights; elsewhere the editor draws its own
 * menu bar and handles the shortcuts.
 */
export function isMacPlatform(
  nav: { readonly platform?: string } | undefined = typeof navigator === "undefined"
    ? undefined
    : navigator,
): boolean {
  return /mac/i.test(nav?.platform ?? "");
}

/** The desktop app shows a native menu (macOS): its accelerators, not the page, run commands. */
export function hasNativeMenu(desktop: DesktopBridge | null = desktopFromWindow()): boolean {
  return desktop?.menu !== undefined && isMacPlatform();
}

/** `window.cutweave` when the editor runs inside the desktop app, else `null`. */
export function desktopFromWindow(
  target: { cutweave?: DesktopBridge } = globalThis as never,
): DesktopBridge | null {
  return target.cutweave ?? null;
}

const SESSION_COMMANDS: readonly string[] = ["new", "open", "save", "saveAs", "undo", "redo"];

export function isSessionCommand(command: string): command is SessionMenuCommand {
  return SESSION_COMMANDS.includes(command);
}
