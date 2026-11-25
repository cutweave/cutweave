// SPDX-License-Identifier: GPL-3.0-or-later
// Engine refusals of timeline edits, said the way the design system asks (design/README.md
// "Errors": what happened and the next step, no apology). The engine's message is the fallback.
import type { RpcError } from "@cutweave/protocol";
import type { TKey } from "../i18n/index.ts";

/** The edits the timeline reports on. */
export type TimelineEdit = "move" | "trim" | "split" | "delete" | "insert" | "rename";

export const FAILED_TITLE: Readonly<Record<TimelineEdit, TKey>> = {
  move: "timeline.failed.move",
  trim: "timeline.failed.trim",
  split: "timeline.failed.split",
  delete: "timeline.failed.delete",
  insert: "timeline.failed.insert",
  rename: "timeline.failed.rename",
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

/** Why the engine refused, for the toast body. */
export function describeRefusal(error: RpcError, t: Translate): string {
  switch (error.code) {
    case "locked":
      return t("timeline.reason.locked");
    case "conflict":
      return error.data?.reason === "item_in_gesture"
        ? t("timeline.reason.inGesture")
        : t("timeline.reason.changed");
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
