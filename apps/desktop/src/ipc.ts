// SPDX-License-Identifier: GPL-3.0-or-later
// IPC channel registry: the single allowlist of channels between main and the renderer.
// Every `invoke` channel has a validator for its arguments; there is no generic
// `invoke(channel, ...)` on the bridge. WP-1L.02 hardens this (zod schemas, sender-frame origin).

import {
  type AppInfo,
  type EngineConnection,
  type EngineStatus,
  MENU_COMMANDS,
  type MenuCommand,
} from "./shared/api.ts";

/** Renderer -> main request/response channels (`ipcMain.handle`). */
export interface InvokeChannels {
  "app:getInfo": { args: []; result: AppInfo };
  "engine:getConnection": { args: []; result: EngineConnection | null };
  "engine:getStatus": { args: []; result: EngineStatus };
  "dialog:openProjectFolder": { args: []; result: string | null };
  "shell:showItemInFolder": { args: [path: string]; result: undefined };
}

/** Main -> renderer events (`webContents.send`). */
export interface EventChannels {
  "engine:status": EngineStatus;
  "menu:command": MenuCommand;
}

export type InvokeChannel = keyof InvokeChannels;
export type EventChannel = keyof EventChannels;

export class IpcValidationError extends Error {
  constructor(channel: string, detail: string) {
    super(`invalid IPC call on ${channel}: ${detail}`);
    this.name = "IpcValidationError";
  }
}

function noArgs(channel: string, args: readonly unknown[]): [] {
  if (args.length !== 0) throw new IpcValidationError(channel, "expected no arguments");
  return [];
}

/** Argument validators, one per invoke channel. They return the typed tuple or throw. */
export const invokeValidators: {
  [K in InvokeChannel]: (args: readonly unknown[]) => InvokeChannels[K]["args"];
} = {
  "app:getInfo": (args) => noArgs("app:getInfo", args),
  "engine:getConnection": (args) => noArgs("engine:getConnection", args),
  "engine:getStatus": (args) => noArgs("engine:getStatus", args),
  "dialog:openProjectFolder": (args) => noArgs("dialog:openProjectFolder", args),
  "shell:showItemInFolder": (args) => {
    const [path, ...rest] = args;
    if (typeof path !== "string" || path.length === 0 || path.length > 4096 || rest.length > 0) {
      throw new IpcValidationError("shell:showItemInFolder", "expected one non-empty path string");
    }
    return [path];
  },
};

export const INVOKE_CHANNELS = Object.keys(invokeValidators) as InvokeChannel[];

export function isMenuCommand(value: unknown): value is MenuCommand {
  return typeof value === "string" && (MENU_COMMANDS as readonly string[]).includes(value);
}

export type InvokeHandlers = {
  [K in InvokeChannel]: (
    ...args: InvokeChannels[K]["args"]
  ) => InvokeChannels[K]["result"] | Promise<InvokeChannels[K]["result"]>;
};

/** Minimal slice of `ipcMain` that `registerIpc` needs (lets tests pass a fake). */
export interface IpcMainLike {
  handle(
    channel: string,
    listener: (event: unknown, ...args: unknown[]) => unknown | Promise<unknown>,
  ): void;
  removeHandler(channel: string): void;
}

/**
 * Register every allowlisted channel with validated arguments. Returns a disposer.
 * WP-1L.02 adds the sender-origin check in `listener`.
 */
export function registerIpc(ipc: IpcMainLike, handlers: InvokeHandlers): () => void {
  for (const channel of INVOKE_CHANNELS) {
    const validate = invokeValidators[channel] as (args: readonly unknown[]) => unknown[];
    const handler = handlers[channel] as (...args: unknown[]) => unknown;
    ipc.handle(channel, (_event, ...args) => handler(...validate(args)));
  }
  return () => {
    for (const channel of INVOKE_CHANNELS) ipc.removeHandler(channel);
  };
}
