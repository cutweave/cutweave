// SPDX-License-Identifier: GPL-3.0-or-later
// The part of `window.cutweave` (apps/desktop `shared/api.ts`) the project flows use: native
// folder picking, the recent-projects list that main keeps, the native menu commands, and the
// window-close handshake (main asks, the editor answers once unsaved work is settled). `null`
// where there is no desktop shell (plain `vite` dev, Vitest): the flows then have no native
// dialogs and the shortcuts are handled by the page itself.

/** Menu commands main sends (a subset of the desktop `MenuCommand`). */
export type SessionMenuCommand = "new" | "open" | "save" | "saveAs" | "undo" | "redo";

export interface DesktopBridge {
  readonly dialog: {
    /** Native folder picker; `null` when cancelled. */
    openProjectFolder(): Promise<string | null>;
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
  };
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
