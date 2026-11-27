// SPDX-License-Identifier: GPL-3.0-or-later
// The keyframe's menu on the timeline (WP-2C.05, docs/research/09-context-menus.md): a key in an
// open keyframe lane, or a diamond of a clip's own key row (every property's key at that time).
// Interpolation ▸ (Linear, Hold, Ease in, Ease out, Ease in-out, Bezier…), Copy, Delete keyframe,
// Go to keyframe. It acts on the selected keys when the key under the pointer is one of them.
import type { Easing, Interp } from "@cutweave/protocol";
import { type KeyAddress, keysForTarget, storedKeys } from "../../timeline/keyframes.ts";
import type { LabelSpec, MenuContext, MenuRow, MenuSurfaceDef, MenuTarget } from "../types.ts";
import { clipFact } from "./facts.ts";

const LOCKED = "contextMenu.reason.trackLocked" as const;
const GONE = "contextMenu.reason.noKeyframes" as const;

/** What the canvas says about the key it was opened on (`menus/canvas.ts`). */
export interface KeyframeData {
  readonly keyframeTime: number | null;
  readonly prop: string | null;
}

/** The clip and keys a keyframe menu acts on, or null when the key is gone. */
export function keyTarget(target: MenuTarget) {
  const data = target.data as KeyframeData | undefined;
  const item = target.ids[0];
  if (item === undefined || data === undefined || data.keyframeTime == null) return null;
  const found = keysForTarget(item, data.prop ?? null, data.keyframeTime);
  return found === null || found.keys.length === 0 ? null : { item, ...found };
}

const locked = (ctx: MenuContext): boolean =>
  clipFact(ctx.target.ids[0] ?? "")?.track.locked === true;
const usable = (ctx: MenuContext): boolean => keyTarget(ctx.target) !== null && !locked(ctx);
const reason = (ctx: MenuContext) => (locked(ctx) ? LOCKED : GONE);

/** The interpolation of the first key the menu acts on. */
function interpOf(ctx: MenuContext): Interp | null {
  const found = keyTarget(ctx.target);
  const key: KeyAddress | undefined = found?.keys[0];
  if (found === null || key === undefined) return null;
  return storedKeys(found.clip, key.prop)?.find((k) => k.t === key.t)?.interp ?? null;
}

/** Whether the first key's segment is exactly this easing (the submenu's radio mark). */
function isEasing(ctx: MenuContext, easing: Easing): boolean {
  const interp = interpOf(ctx);
  if (interp === null) return false;
  if (easing === "hold" || easing === "linear") return interp.kind === easing;
  return interp.kind === "bezier" && interp.preset === easing;
}

const easingRow = (easing: Easing, label: LabelSpec, order: number): MenuRow => ({
  id: `easing-${easing}`,
  command: "keyframe.set_interp",
  group: "easing",
  order,
  label,
  radio: true,
  checked: (ctx) => isEasing(ctx, easing),
  enabledIf: usable,
  reason,
  args: (ctx) => {
    const found = keyTarget(ctx.target);
    return { item: found?.item, keys: found?.keys, easing };
  },
});

/** The key the dialog edits: the first the menu acts on. */
export const curveArgs = (ctx: MenuContext) => {
  const found = keyTarget(ctx.target);
  const key = found?.keys[0];
  return key === undefined ? undefined : { item: found?.item, prop: key.prop, t: key.t };
};

export const TIMELINE_KEYFRAME: MenuSurfaceDef = {
  id: "timeline.keyframe",
  name: () => ({ key: "contextMenu.name.keyframe" }),
  groups: ["easing", "copy", "remove", "navigate"],
  rows: [
    {
      id: "interpolation",
      group: "easing",
      order: 0,
      submenu: {
        label: { key: "contextMenu.item.interpolation" },
        rows: [
          easingRow("linear", { key: "contextMenu.item.easingLinear" }, 0),
          easingRow("hold", { key: "contextMenu.item.easingHold" }, 1),
          easingRow("ease_in", { key: "contextMenu.item.easingEaseIn" }, 2),
          easingRow("ease_out", { key: "contextMenu.item.easingEaseOut" }, 3),
          easingRow("ease_in_out", { key: "contextMenu.item.easingEaseInOut" }, 4),
          {
            command: "timeline.edit_keyframe_curve",
            group: "custom",
            order: 0,
            label: { key: "contextMenu.item.easingBezier" },
            enabledIf: usable,
            reason,
            args: curveArgs,
          },
        ],
      },
    },
    {
      command: "timeline.copy_keyframes",
      group: "copy",
      order: 0,
      enabledIf: (ctx) => keyTarget(ctx.target) !== null,
      reason: GONE,
    },
    {
      id: "delete-keyframe",
      command: "keyframe.delete",
      group: "remove",
      order: 0,
      variant: "danger",
      label: (ctx) => {
        const count = keyTarget(ctx.target)?.keys.length ?? 1;
        return count > 1
          ? { key: "contextMenu.item.deleteKeyframes", vars: { count } }
          : { key: "contextMenu.item.deleteKeyframe" };
      },
      enabledIf: usable,
      reason,
      args: (ctx) => {
        const found = keyTarget(ctx.target);
        return { item: found?.item, keys: found?.keys };
      },
    },
    {
      command: "timeline.go_to_keyframe",
      group: "navigate",
      order: 0,
      args: (ctx) => ({ t: (ctx.target.data as KeyframeData | undefined)?.keyframeTime }),
    },
  ],
};
