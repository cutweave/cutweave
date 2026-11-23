// SPDX-License-Identifier: GPL-3.0-or-later
// Engine refusals of project actions, said the way the design system asks (design/README.md
// "Errors": what happened and the next step, no apology). The engine's message is the fallback.
import { type RpcError, RpcError as RpcErrorClass } from "@cutweave/protocol";
import type { Translate } from "../timeline/errors.ts";

export function asRpcError(error: unknown): RpcError {
  return error instanceof RpcErrorClass ? error : new RpcErrorClass("internal", String(error));
}

/** Why a project command was refused, for a toast body. */
export function describeProjectError(error: RpcError, t: Translate): string {
  const message = error.message;
  if (/open in another Cutweave process/i.test(message)) return t("project.reason.locked");
  if (/no \.cutweave project document found|several \.cutweave documents/i.test(message)) {
    return t("project.reason.missing");
  }
  if (/project already exists/i.test(message)) return t("project.reason.exists");
  if (error.code === "unsupported" && /read-only/i.test(message)) {
    return t("project.reason.readOnly");
  }
  return t("project.reason.other", { message });
}

/** HH:MM (24 hour) of a wall-clock time in milliseconds, for "last at 14:05". */
export function clockTime(wallMs: number): string {
  const d = new Date(wallMs);
  const pad = (n: number) => (n < 10 ? `0${n}` : String(n));
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
