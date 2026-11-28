// SPDX-License-Identifier: GPL-3.0-or-later
// The graph editor's edits as engine commands (WP-2C.07). Nothing here is new to the engine:
//
//   keys dragged in time            `keyframe.move{item, keys, delta}`
//   keys dragged in value           `keyframe.set{item, prop, keys:[{t, value}]}` per property
//                                   (a key at an occupied time keeps its easing)
//   a bezier handle dragged         `keyframe.set_interp{item, keys:[key], interp}`: the out
//                                   handle is (x1, y1) of the key's own segment, the in handle is
//                                   (x2, y2) of the segment before it; aligned and mirrored
//                                   tangents also write the segment on the key's other side
//
// A drag sends these as steps of one gesture transaction (graphGesture.ts); the arrow keys and
// the Time and Value fields send them once. Keys are addressed by their stored times as they
// were when the edit began.
import type { Interp, KeyRef, PropId } from "@cutweave/protocol";
import type { GraphEdit } from "@cutweave/ui";
import type { KeyAddress } from "../../timeline/keyframes.ts";
import type { GraphStep, StepSend } from "./graphGesture.ts";
import { clampToProp, type EditorCurve, indexOfTime, type StoredGraphKey } from "./graphModel.ts";

/** The clip, the selected keys and the curves as they were when an edit began. */
export interface EditOrigin {
  readonly item: string;
  readonly keys: readonly KeyAddress[];
  readonly curves: readonly EditorCurve[];
}

export interface Command {
  readonly name: string;
  readonly args: Record<string, unknown>;
}

type Value = StoredGraphKey["value"];

const refs = (keys: readonly KeyAddress[]): KeyRef[] =>
  keys.map((k) => ({ prop: k.prop as PropId, t: k.t }));

/** Rounds away the noise a sum of binary fractions leaves (0.30000000000000004). */
const tidy = (v: number): number => Number(v.toPrecision(12));

/**
 * The new values of the selected keys when the keys of curve `id` change by `dv[id]`, as
 * `prop -> stored time -> value`. A position key changes only in the component whose curve
 * moved; a flip has no value to drag.
 */
export function movedValues(
  origin: EditOrigin,
  change: (curve: EditorCurve, old: number) => number | null,
): Map<string, Map<number, Value>> {
  const out = new Map<string, Map<number, Value>>();
  for (const curve of origin.curves) {
    if (curve.item !== origin.item || curve.shape === "bool" || curve.locked === true) continue;
    for (const key of origin.keys) {
      if (key.prop !== curve.prop) continue;
      const index = indexOfTime(curve.itemTimes, key.t);
      if (index < 0) continue;
      const next = change(curve, curve.values[index] as number);
      if (next === null) continue;
      const clamped = clampToProp(curve.prop, tidy(next));
      let byTime = out.get(curve.prop);
      if (byTime === undefined) {
        byTime = new Map();
        out.set(curve.prop, byTime);
      }
      if (curve.shape === "scalar") byTime.set(key.t, clamped);
      else {
        const stored = (byTime.get(key.t) ?? (curve.keys[index] as StoredGraphKey).value) as {
          x: number;
          y: number;
        };
        byTime.set(key.t, curve.comp === 0 ? { ...stored, x: clamped } : { ...stored, y: clamped });
      }
    }
  }
  return out;
}

/** What a step that moved keys resolves with. */
export interface MoveOutput {
  /** Where the selected keys are now. */
  readonly keys: readonly KeyAddress[];
  /** How far they moved in time (flicks, whole frames). */
  readonly shift: number;
}

interface MovedReply {
  readonly moved?: readonly {
    readonly prop: string;
    readonly from_t: number;
    readonly t: number;
  }[];
}

/** Sends a move in time and value; resolves with where the keys are. */
async function sendMove(
  send: StepSend,
  origin: EditOrigin,
  dt: number,
  values: Map<string, Map<number, Value>>,
): Promise<MoveOutput> {
  const now = new Map<string, number>();
  const at = (k: KeyAddress): number => now.get(`${k.prop}|${k.t}`) ?? k.t;
  let shift = 0;
  if (dt !== 0) {
    const reply = (await send("keyframe.move", {
      item: origin.item,
      keys: refs(origin.keys),
      delta: dt,
    })) as MovedReply | null;
    for (const m of reply?.moved ?? []) {
      now.set(`${m.prop}|${m.from_t}`, m.t);
      shift = m.t - m.from_t;
    }
  }
  for (const [prop, byTime] of values) {
    const keys = [...byTime].map(([t, value]) => ({ t: at({ prop, t }), value }));
    await send("keyframe.set", { item: origin.item, prop, keys });
  }
  return { keys: origin.keys.map((k) => ({ prop: k.prop, t: at(k) })), shift };
}

/**
 * A step that changes nothing: it writes the first selected key's own value, which rolls the
 * gesture's earlier steps back (the pointer is back where the drag began).
 */
function restStep(origin: EditOrigin): GraphStep | null {
  for (const key of origin.keys) {
    const curve = origin.curves.find((c) => c.item === origin.item && c.prop === key.prop);
    const index = curve === undefined ? -1 : indexOfTime(curve.itemTimes, key.t);
    if (curve === undefined || index < 0) continue;
    const value = (curve.keys[index] as StoredGraphKey).value;
    return {
      key: "rest",
      run: async (send) => {
        await send("keyframe.set", {
          item: origin.item,
          prop: key.prop,
          keys: [{ t: key.t, value }],
        });
        return { keys: origin.keys, shift: 0 } satisfies MoveOutput;
      },
    };
  }
  return null;
}

/** The step of a drag of the selected keys by `dt` in time and `dv[curve]` in value. */
export function moveStep(
  origin: EditOrigin,
  edit: Extract<GraphEdit, { type: "move" }>,
): GraphStep | null {
  if (origin.keys.length === 0) return null;
  const values = movedValues(origin, (curve, old) => {
    const dv = edit.dv[curve.id];
    return dv === undefined || dv === 0 ? null : old + dv;
  });
  if (edit.dt === 0 && values.size === 0) return restStep(origin);
  return {
    key: `move:${edit.dt}:${JSON.stringify(edit.dv)}`,
    run: (send) => sendMove(send, origin, edit.dt, values),
  };
}

/** The commands of a handle drag: the key's own segment, and the one across it when tied. */
export function tangentCommands(
  origin: EditOrigin,
  edit: Extract<GraphEdit, { type: "tangent" }>,
): Command[] {
  const curve = origin.curves.find((c) => c.id === edit.curve);
  if (curve === undefined) return [];
  const at = (index: number): { t: number; interp: Interp } | null => {
    const key = curve.keys[index];
    return key === undefined ? null : { t: key.t, interp: key.interp };
  };
  const own = at(edit.side === "out" ? edit.index : edit.index - 1);
  const other = at(edit.side === "out" ? edit.index - 1 : edit.index);
  const out: Command[] = [];
  const set = (key: { t: number; interp: Interp } | null, patch: Partial<Interp>): void => {
    if (key === null || key.interp.kind !== "bezier") return;
    const { x1, y1, x2, y2 } = key.interp;
    out.push({
      name: "keyframe.set_interp",
      args: {
        item: curve.item,
        keys: [{ prop: curve.prop, t: key.t }],
        // The numbers alone: a hand-shaped curve is no longer the preset it came from.
        interp: { kind: "bezier", x1, y1, x2, y2, ...patch },
      },
    });
  };
  if (edit.side === "out") {
    set(own, { x1: edit.x, y1: edit.y } as Partial<Interp>);
    if (edit.opposite !== null) {
      set(other, { x2: edit.opposite.x, y2: edit.opposite.y } as Partial<Interp>);
    }
  } else {
    set(own, { x2: edit.x, y2: edit.y } as Partial<Interp>);
    if (edit.opposite !== null) {
      set(other, { x1: edit.opposite.x, y1: edit.opposite.y } as Partial<Interp>);
    }
  }
  return out;
}

export function tangentStep(
  origin: EditOrigin,
  edit: Extract<GraphEdit, { type: "tangent" }>,
): GraphStep | null {
  const commands = tangentCommands(origin, edit);
  if (commands.length === 0) return null;
  return {
    key: `tangent:${edit.side}:${edit.x}:${edit.y}:${edit.opposite?.x}:${edit.opposite?.y}`,
    run: async (send) => {
      for (const c of commands) await send(c.name, c.args);
      return null;
    },
  };
}

/** The arrow keys: the selected keys move by `dt` flicks, or their values by `steps` steps. */
export function nudgeStep(
  origin: EditOrigin,
  dt: number,
  dvOf: (curve: EditorCurve) => number,
  shown: (curve: EditorCurve) => boolean,
): GraphStep | null {
  if (origin.keys.length === 0) return null;
  const values = movedValues(origin, (curve, old) => {
    const dv = shown(curve) ? dvOf(curve) : 0;
    return dv === 0 ? null : old + dv;
  });
  if (dt === 0 && values.size === 0) return null;
  return { key: "nudge", run: (send) => sendMove(send, origin, dt, values) };
}

/** The Value field: the selected points all take `value`. */
export function valueStep(
  origin: EditOrigin,
  value: number,
  shown: (curve: EditorCurve) => boolean,
): GraphStep | null {
  const values = movedValues(origin, (curve) => (shown(curve) ? value : null));
  if (values.size === 0) return null;
  return { key: "value", run: (send) => sendMove(send, origin, 0, values) };
}

/** A bezier with the shape of a straight line: handles appear without the curve changing. */
export const STRAIGHT_BEZIER: Interp = {
  kind: "bezier",
  x1: 0.333333,
  y1: 0.333333,
  x2: 0.666667,
  y2: 0.666667,
} as Interp;
