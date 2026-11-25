// SPDX-License-Identifier: GPL-3.0-or-later
// What the menu machinery needs from the app, behind one object so tests (and Storybook) can
// swap it: the registries, the engine call, the keymap, the stores' facts and the translator.
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { sessionStore } from "../session/sessionStore.ts";
import { menuPlatform } from "../shell/appMenu.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { CommandInfoTable, UiCommandTable } from "./commands.ts";
import { keysFor } from "./keys.ts";
import { MenuContributionRegistry, MenuSurfaceRegistry } from "./registry.ts";
import type { LabelSpec } from "./types.ts";
import type { ContextValues } from "./when.ts";

export interface MenuServices {
  readonly commands: CommandInfoTable;
  readonly ui: UiCommandTable;
  readonly surfaces: MenuSurfaceRegistry;
  readonly contributions: MenuContributionRegistry;
  /** Calls a registry command on the engine. */
  call(name: string, args?: unknown): Promise<unknown>;
  /** The shortcut text of a command, from the keymap. */
  keysFor(command: string): string | undefined;
  /** The keys every menu has: `engine.live`, `project.readOnly`. */
  baseKeys(): ContextValues;
  /** The playhead in flicks. */
  playhead(): number | null;
  t(spec: LabelSpec): string;
  /** An unpackaged desktop build: the dead zone offers "Inspect element". */
  isDev(): boolean;
  /** A choice failed: tell the person. */
  report(error: unknown): void;
  /** Milliseconds, monotonic (injectable so tests need no waiting). */
  now(): number;
}

const platform = () => menuPlatform(typeof navigator === "undefined" ? "" : navigator.platform);

let dev = false;
/** Set once the desktop shell has said it is an unpackaged build. */
export function setDevBuild(value: boolean): void {
  dev = value;
}

export function createMenuServices(overrides: Partial<MenuServices> = {}): MenuServices {
  return {
    commands: new CommandInfoTable(),
    ui: new UiCommandTable(),
    surfaces: new MenuSurfaceRegistry(),
    contributions: new MenuContributionRegistry(),
    async call(name, args) {
      const rpc = engineStore.getState().rpc;
      if (rpc === null) throw new Error("the engine is not connected");
      return (rpc.call as (n: string, a: unknown) => Promise<unknown>)(name, args ?? {});
    },
    keysFor: (command) => keysFor(command, platform()),
    baseKeys: () => ({
      "engine.live": engineStore.getState().connection === "ready",
      "project.readOnly": sessionStore.getState().readOnly !== null,
    }),
    playhead: () => viewStore.getState().playhead,
    t: (spec) => i18n.t(spec.key, spec.vars ?? {}) as string,
    isDev: () => dev,
    report(error) {
      toastStore.getState().push({
        tone: "danger",
        title: i18n.t("contextMenu.failed"),
        message: error instanceof Error ? error.message : String(error),
      });
    },
    now: () => (typeof performance === "undefined" ? Date.now() : performance.now()),
    ...overrides,
  };
}
