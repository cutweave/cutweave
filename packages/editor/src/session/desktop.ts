// SPDX-License-Identifier: GPL-3.0-or-later
// The part of `window.cutweave` (apps/desktop `shared/api.ts`) the project flows and the title
// bar use: native folder picking, the recent-projects list that main keeps, the native menu
// commands (macOS), the window-close handshake (main asks, the editor answers once unsaved work
// is settled), and the actions of the in-app menu bar only main can take (WP-1M.09: quit, full
// screen, the edit commands, About, the theme of the OS window controls, and the dev-only reload
// and developer tools). `null` where there is no desktop shell (plain `vite` dev, Vitest): the
// flows then have no native dialogs and the page handles every shortcut itself.

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
  };
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
