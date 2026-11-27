// SPDX-License-Identifier: GPL-3.0-or-later
// IPC registry (arch §18, WP-1L.02): the single allowlist of renderer -> main channels. Every
// handler first checks the sender (top-level frame of an editor window on the app origin), then
// validates its arguments with a zod schema, and only then runs. The preload exposes named
// functions only; there is no generic `invoke(channel, ...)` anywhere on the bridge.

import { z } from "zod";
import {
  isPreferenceKey,
  isRendererWritable,
  validatePreference,
} from "../../../../packages/preferences/src/index.ts";
import {
  AGENT_CLIENTS,
  APP_FOLDERS,
  EDIT_ACTIONS,
  EXPORT_CONTAINERS,
  MAX_NOTICE_BODY,
  MAX_NOTICE_TITLE,
  THEMES,
} from "../shared/api.ts";
import type { InvokeChannel, InvokeChannels } from "../shared/ipc-contract.ts";
import {
  isTrustedOrigin,
  MAX_EXTERNAL_URL_LENGTH,
  type OriginPolicy,
  parseExternalUrl,
} from "./origins.ts";
import { POPOUT_FRAME_NAME } from "./popout.ts";

export class IpcValidationError extends Error {
  constructor(channel: string, detail: string) {
    super(`invalid IPC call on ${channel}: ${detail}`);
    this.name = "IpcValidationError";
  }
}

export class IpcSenderError extends Error {
  constructor(channel: string, reason: string) {
    super(`IPC on ${channel} rejected: ${reason}`);
    this.name = "IpcSenderError";
  }
}

const noArgs = z.tuple([]);

/** A filesystem path from the renderer: non-empty, bounded, no NUL. Main never trusts it further. */
const filePath = z
  .string()
  .min(1)
  .max(4096)
  .refine((p) => !p.includes("\0"), "path must not contain NUL");

/** A link for the OS browser: `https:` only, with a host and no credentials. */
const externalUrl = z
  .string()
  .max(MAX_EXTERNAL_URL_LENGTH)
  .refine((u) => parseExternalUrl(u) !== null, "expected an https: URL without credentials");

/**
 * One preference write: a known key and a value of its type and range. The schema is the one the
 * editor imports (packages/preferences), so a value the dialog cannot produce is refused here.
 * Keys only main writes ("Folders agents can use", WP-1J.05) are refused: the renderer never
 * names a folder agents may use; main's own folder picker adds them (`agents:addFolder`).
 */
const preferenceWrite = z
  .tuple([z.string().max(128), z.unknown()])
  .superRefine(([key, value], context) => {
    if (isPreferenceKey(key) && !isRendererWritable(key)) {
      context.addIssue({ code: "custom", message: `${key}: only the app's own dialogs change it` });
      return;
    }
    const checked = validatePreference(key, value);
    if (!checked.ok) context.addIssue({ code: "custom", message: `${key}: ${checked.reason}` });
  });

/** Argument schemas, one per allowlisted channel; checked against the shared contract types. */
export const invokeSchemas = {
  "app:getInfo": noArgs,
  "engine:getConnection": noArgs,
  "engine:getStatus": noArgs,
  "dialog:openProjectFolder": noArgs,
  "recent:list": noArgs,
  "recent:add": z.tuple([filePath]),
  "recent:remove": z.tuple([filePath]),
  "recent:clear": noArgs,
  "window:setDirty": z.tuple([z.boolean()]),
  "window:confirmClose": noArgs,
  "dialog:pickMediaFiles": noArgs,
  "dialog:pickMediaFolders": noArgs,
  "dialog:chooseExportFile": z.union([
    z.tuple([filePath]),
    z.tuple([filePath, z.enum(EXPORT_CONTAINERS)]),
  ]),
  "shell:showItemInFolder": z.tuple([filePath]),
  "shell:openFile": z.tuple([filePath]),
  // Plain, bounded text for the OS notification; main decides whether it shows.
  "notify:exportFinished": z.tuple([
    z.strictObject({
      title: z.string().min(1).max(MAX_NOTICE_TITLE),
      body: z.string().max(MAX_NOTICE_BODY),
    }),
  ]),
  "shell:openExternal": z.tuple([externalUrl]),
  "app:about": noArgs,
  "app:quit": noArgs,
  "window:toggleFullScreen": noArgs,
  "window:setTheme": z.tuple([z.enum(THEMES)]),
  "window:reload": noArgs,
  "window:toggleDevTools": noArgs,
  "window:inspectElement": z.tuple([
    z.number().int().min(0).max(100_000),
    z.number().int().min(0).max(100_000),
  ]),
  // A detached panel's window by dockview's name for it, and nothing else (ADR-0031).
  "window:setPopoutOnTop": z.tuple([
    z.strictObject({ frame: z.string().regex(POPOUT_FRAME_NAME), on: z.boolean() }),
  ]),
  "edit:run": z.tuple([z.enum(EDIT_ACTIONS)]),
  "prefs:get": noArgs,
  "prefs:set": preferenceWrite,
  "prefs:reset": z.tuple([z.array(z.string().refine(isPreferenceKey, "unknown setting")).max(200)]),
  // `"shortcuts"`: only the keyboard shortcuts (WP-2H.05).
  "prefs:export": z.union([noArgs, z.tuple([z.literal("shortcuts")])]),
  "prefs:import": z.union([noArgs, z.tuple([z.literal("shortcuts")])]),
  "shell:openAppFolder": z.tuple([z.enum(APP_FOLDERS)]),
  "agents:addFolder": noArgs,
  "agents:removeFolder": z.tuple([filePath]),
  "agents:connectInfo": noArgs,
  "agents:copySetup": z.tuple([z.enum(AGENT_CLIENTS)]),
} as const satisfies { [K in InvokeChannel]: z.ZodType<InvokeChannels[K]["args"]> };

export const INVOKE_CHANNELS = Object.keys(invokeSchemas) as InvokeChannel[];

/** Validate the arguments of one call; returns the typed tuple or throws IpcValidationError. */
export function parseInvokeArgs<K extends InvokeChannel>(
  channel: K,
  args: readonly unknown[],
): InvokeChannels[K]["args"] {
  const schema = invokeSchemas[channel] as z.ZodType<InvokeChannels[K]["args"]> | undefined;
  if (schema === undefined) throw new IpcValidationError(channel, "unknown channel");
  const result = schema.safeParse(args);
  if (!result.success) throw new IpcValidationError(channel, z.prettifyError(result.error));
  return result.data;
}

export type InvokeHandlers = {
  [K in InvokeChannel]: (
    ...args: InvokeChannels[K]["args"]
  ) => InvokeChannels[K]["result"] | Promise<InvokeChannels[K]["result"]>;
};

/** The parts of an `IpcMainInvokeEvent` the sender check reads. */
export interface IpcEventLike {
  readonly sender: unknown;
  readonly senderFrame: { readonly url: string; readonly parent: unknown } | null;
}

/** Returns null when the sender may call main, or the reason it may not. */
export type SenderCheck = (event: IpcEventLike) => string | null;

export interface SenderPolicy extends OriginPolicy {
  /** True when `sender` is the webContents of a window the factory created as `editor`. */
  isEditorContents(sender: unknown): boolean;
}

/**
 * Sender-frame check run before every handler: the frame must still exist, be the top-level frame
 * (never an iframe), be on the app origin (dev server in dev), and belong to an editor window.
 */
export function createSenderCheck(policy: SenderPolicy): SenderCheck {
  return (event) => {
    const frame = event.senderFrame;
    if (frame === null) return "the sender frame is gone";
    if (frame.parent !== null) return "IPC from a subframe";
    if (!isTrustedOrigin(frame.url, policy)) return "untrusted sender origin";
    if (!policy.isEditorContents(event.sender)) return "the sender is not an editor window";
    return null;
  };
}

/** Minimal slice of `ipcMain` that `registerIpc` needs (lets tests pass a fake). */
export interface IpcMainLike {
  handle(
    channel: string,
    listener: (event: IpcEventLike, ...args: unknown[]) => unknown | Promise<unknown>,
  ): void;
  removeHandler(channel: string): void;
}

/**
 * Register every allowlisted channel: sender check, then zod validation, then the handler.
 * Unlisted channels have no handler, so Electron rejects them. Returns a disposer.
 */
export function registerIpc(
  ipc: IpcMainLike,
  handlers: InvokeHandlers,
  checkSender: SenderCheck,
): () => void {
  for (const channel of INVOKE_CHANNELS) {
    const handler = handlers[channel] as (...args: unknown[]) => unknown;
    ipc.handle(channel, (event, ...args) => {
      const reason = checkSender(event);
      if (reason !== null) throw new IpcSenderError(channel, reason);
      return handler(...parseInvokeArgs(channel, args));
    });
  }
  return () => {
    for (const channel of INVOKE_CHANNELS) ipc.removeHandler(channel);
  };
}
