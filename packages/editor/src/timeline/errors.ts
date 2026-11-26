// SPDX-License-Identifier: GPL-3.0-or-later
// Engine refusals of timeline edits, said the way the design system asks (design/README.md
// "Errors": what happened and the next step, no apology). The engine's message is the fallback.
import { isTransactionLost, type RpcError } from "@cutweave/protocol";
import type { TKey } from "../i18n/index.ts";
import { actorDisplayName } from "../session/historyLabels.ts";

/** The edits the timeline reports on. */
export type TimelineEdit =
  | "move"
  | "trim"
  | "split"
  | "delete"
  | "insert"
  | "rename"
  | "copy"
  | "cut"
  | "paste"
  | "duplicate";

export const FAILED_TITLE: Readonly<Record<TimelineEdit, TKey>> = {
  move: "timeline.failed.move",
  trim: "timeline.failed.trim",
  split: "timeline.failed.split",
  delete: "timeline.failed.delete",
  insert: "timeline.failed.insert",
  rename: "timeline.failed.rename",
  copy: "timeline.failed.copy",
  cut: "timeline.failed.cut",
  paste: "timeline.failed.paste",
  duplicate: "timeline.failed.duplicate",
};

export type Translate = (key: TKey, vars?: Readonly<Record<string, string>>) => string;

/**
 * A `Translate` over i18next's `t`. Its typed overloads cannot take a key union together with
 * interpolation values, so the call goes through the plain signature; keys stay checked by `TKey`.
 */
export function translator(t: unknown): Translate {
  const plain = t as (key: string, vars?: Readonly<Record<string, string>>) => string;
  return (key, vars) => plain(key, vars);
}

/** The soft-lock reason of a `conflict` (arch §5.2): an item in another actor's gesture or proposal. */
const IN_GESTURE = "item_in_gesture";

/** Who holds the clip a refused edit touched (`Conflict{item_in_gesture, holder}`, WP-2A.12). */
export function lockHolder(error: RpcError): string | null {
  if (error.code !== "conflict" || error.data?.reason !== IN_GESTURE) return null;
  const holder = error.data?.holder;
  return typeof holder === "string" ? holder : null;
}

/** Whether an agent, a plugin or the engine (not the person) holds the clip the edit touched. */
export function heldByOther(error: RpcError): boolean {
  const holder = lockHolder(error);
  return holder !== null && holder !== "user";
}

/** Why the engine refused, for the toast body. */
export function describeRefusal(error: RpcError, t: Translate): string {
  switch (error.code) {
    case "locked":
      return t("timeline.reason.locked");
    case "conflict": {
      // The engine restarted under the gesture: its transaction died with the socket.
      if (isTransactionLost(error)) return t("timeline.reason.restarted");
      if (error.data?.reason !== IN_GESTURE) return t("timeline.reason.changed");
      // Who holds it, and the machine code (WP-2A.12); the editor never retries on its own.
      const holder = lockHolder(error);
      const who = holder === null ? null : actorDisplayName(holder, t);
      return who === null
        ? t("timeline.reason.inGesture", { code: IN_GESTURE })
        : t("timeline.reason.inUseBy", { who, code: IN_GESTURE });
    }
    case "not_found":
      // The gesture's transaction is gone: idle past the engine's timeout, or rolled back.
      if (error.data?.kind === "transaction" || /^transaction\b/i.test(error.message)) {
        return t("timeline.reason.interrupted");
      }
      return t("timeline.reason.missing");
    case "invalid":
      if (/overlap/i.test(error.message)) return t("timeline.reason.overlap");
      return t("timeline.reason.other", { message: error.message });
    default:
      return t("timeline.reason.other", { message: error.message });
  }
}
