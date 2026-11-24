// SPDX-License-Identifier: GPL-3.0-or-later
// The questions the project flows ask a person (WP-1M.07): "Save changes?" before something
// that would lose them, and the name and place of a new or copied project. A flow awaits the
// answer (`ProjectController`), the dialog component (`ProjectDialogs`) resolves it.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export type UnsavedChoice = "save" | "discard" | "cancel";

/** What the person chose for a new project: its folder is `<parent>/<name>`. */
export interface ProjectTarget {
  readonly name: string;
  /** The folder to create the project folder in. */
  readonly parent: string;
  /** The project folder: `parent` + separator + `name`. */
  readonly path: string;
}

export interface UnsavedPrompt {
  /** The project's name, for the title. */
  readonly project: string;
  resolve(choice: UnsavedChoice): void;
}

export interface NamingPrompt {
  readonly mode: "new" | "saveAs";
  readonly initialName: string;
  resolve(target: ProjectTarget | null): void;
}

export interface SessionUiStore {
  readonly unsaved: UnsavedPrompt | null;
  readonly naming: NamingPrompt | null;
  /** Recent projects, newest first (main keeps them; the top bar menu lists them). */
  readonly recent: readonly string[];
  /** A project action is running (open, save, create): buttons wait instead of racing it. */
  readonly busy: boolean;
  setUnsaved(prompt: UnsavedPrompt | null): void;
  setNaming(prompt: NamingPrompt | null): void;
  setRecent(recent: readonly string[]): void;
  setBusy(busy: boolean): void;
}

export function createSessionUiStore() {
  return createStore<SessionUiStore>()((set) => ({
    unsaved: null,
    naming: null,
    recent: [],
    busy: false,
    setUnsaved: (unsaved) => set({ unsaved }),
    setNaming: (naming) => set({ naming }),
    setRecent: (recent) => set({ recent }),
    setBusy: (busy) => set({ busy }),
  }));
}

export const sessionUiStore = createSessionUiStore();

export function useSessionUiStore<T>(selector: (state: SessionUiStore) => T): T {
  return useStore(sessionUiStore, selector);
}

const INVALID_NAME = /[\\/:*?"<>|]/;

/** A control character (below U+0020) cannot be part of a folder name. */
const hasControlCharacter = (s: string): boolean => [...s].some((ch) => ch.charCodeAt(0) < 32);

export type NameProblem = "empty" | "invalid" | "long";

/** Why a project name cannot be a folder name, or `null` when it can. */
export function projectNameProblem(name: string): NameProblem | null {
  const trimmed = name.trim();
  if (trimmed === "") return "empty";
  if (trimmed.length > 200) return "long";
  if (
    INVALID_NAME.test(trimmed) ||
    hasControlCharacter(trimmed) ||
    trimmed.endsWith(".") ||
    trimmed.startsWith(".")
  ) {
    return "invalid";
  }
  return null;
}

/** `parent` + the path separator it uses + `name`. */
export function joinProjectPath(parent: string, name: string): string {
  const sep = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
  return parent.endsWith("/") || parent.endsWith("\\")
    ? `${parent}${name}`
    : `${parent}${sep}${name}`;
}
