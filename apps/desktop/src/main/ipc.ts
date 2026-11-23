// SPDX-License-Identifier: GPL-3.0-or-later
// IPC registry (arch §18, WP-1L.02): the single allowlist of renderer -> main channels. Every
// handler first checks the sender (top-level frame of an editor window on the app origin), then
// validates its arguments with a zod schema, and only then runs. The preload exposes named
// functions only; there is no generic `invoke(channel, ...)` anywhere on the bridge.

import { z } from "zod";
import type { InvokeChannel, InvokeChannels } from "../shared/ipc-contract.ts";
import {
  isTrustedOrigin,
  MAX_EXTERNAL_URL_LENGTH,
  type OriginPolicy,
  parseExternalUrl,
} from "./origins.ts";

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

/** Argument schemas, one per allowlisted channel; checked against the shared contract types. */
export const invokeSchemas = {
  "app:getInfo": noArgs,
  "engine:getConnection": noArgs,
  "engine:getStatus": noArgs,
  "dialog:openProjectFolder": noArgs,
  "recent:list": noArgs,
  "recent:add": z.tuple([filePath]),
  "recent:remove": z.tuple([filePath]),
  "window:setDirty": z.tuple([z.boolean()]),
  "window:confirmClose": noArgs,
  "dialog:pickMediaFiles": noArgs,
  "shell:showItemInFolder": z.tuple([filePath]),
  "shell:openExternal": z.tuple([externalUrl]),
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
