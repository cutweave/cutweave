// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.08 property test (fast-check): for random event streams (pointer, keys, frames, card
// drags, the engine's answers and cancellations, other actors' edits, tool and preference
// changes) the machine
//   - is always in a documented state, holding a transaction exactly in the gesture states;
//   - never leaves a `gesture` transaction open after pointer-up, pointer-cancel or blur;
//   - pairs every `begin_tx` with exactly one `commit_tx` or `cancel_tx` (the fake engine
//     throws on an update or a close for a gesture that is not open, and on a second begin);
//   - sends at most one update per animation frame (only a release flushes one more, right
//     before its commit);
//   - takes only edges of the documented state chart.
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath } from "@cutweave/protocol/wasm";
import fc from "fast-check";
import { beforeAll, describe, expect, it } from "vitest";
import { GESTURE_STATES, MACHINE_STATES, type StateName } from "./state.ts";
import { edgeKey, readChart } from "./testing/chart.ts";
import { AUDIO, IMAGE, MODEL, VIDEO, World, without } from "./testing/world.ts";
import { type MachineAction, type MachineEvent, type Modifiers, TOOLS } from "./types.ts";

beforeAll(async () => {
  await initMath(mathModuleBytes());
}, 900_000);

/** An event, or an engine answer resolved against the gesture open when it is sent. */
type Op =
  | { readonly kind: "event"; readonly event: MachineEvent }
  | {
      readonly kind: "result";
      readonly ok: boolean;
      readonly code: string;
      readonly clamped: boolean;
    }
  | { readonly kind: "cancelled"; readonly stale: boolean }
  | { readonly kind: "model"; readonly drop: readonly string[] };

const rarely = fc.oneof(
  { weight: 5, arbitrary: fc.constant(false) },
  { weight: 1, arbitrary: fc.constant(true) },
);
const mods: fc.Arbitrary<Modifiers> = fc.record({
  shift: rarely,
  alt: rarely,
  ctrl: rarely,
  meta: rarely,
});

/** Points worth hitting: clip bodies and edges, keyframes, lanes, the ruler, headers, edges. */
function hotspots(): { x: number; y: number }[] {
  const w = new World();
  const out: { x: number; y: number }[] = [];
  for (const it of MODEL.items) {
    const y = w.yOf(it.trackId);
    const x0 = w.xOf(it.start);
    const x1 = w.xOf(it.start + it.duration);
    out.push({ x: x0 + 2, y }, { x: (x0 + x1) / 2, y }, { x: x1 - 2, y });
    for (const k of it.keyframes ?? []) out.push({ x: w.xOf(k), y: w.keyframeY(it.trackId) });
  }
  for (const t of MODEL.tracks) out.push({ x: 50, y: w.yOf(t.id) }, { x: 900, y: w.yOf(t.id) });
  out.push({ x: 300, y: 10 }, { x: 700, y: 10 }, { x: 995, y: 200 }, { x: 105, y: 200 });
  return out;
}

const SPOTS = hotspots();

const where = fc.oneof(
  {
    weight: 3,
    arbitrary: fc
      .tuple(
        fc.constantFrom(...SPOTS),
        fc.integer({ min: -12, max: 12 }),
        fc.integer({ min: -6, max: 6 }),
      )
      .map(([p, dx, dy]) => ({ x: p.x + dx, y: p.y + dy })),
  },
  {
    weight: 1,
    arbitrary: fc.record({
      x: fc.integer({ min: -30, max: 1030 }),
      y: fc.integer({ min: -10, max: 400 }),
    }),
  },
);

const ACTIONS: readonly MachineAction[] = [
  "cancel",
  "split",
  "delete",
  "ripple_delete",
  "toggle_snapping",
  "nudge_left",
  "nudge_right_10",
  "select_all",
  "deselect",
  ...TOOLS.map((t) => `tool.${t}` as const),
];

const KEYS = [
  "Escape",
  "s",
  "Delete",
  "Backspace",
  "n",
  ",",
  ".",
  "v",
  "b",
  "r",
  "y",
  "u",
  "c",
  "h",
  "z",
  "q",
];
const MODIFIER_NAMES = ["Shift", "Alt", "Control", "Meta"];

const event = (e: fc.Arbitrary<MachineEvent>): fc.Arbitrary<Op> =>
  e.map((ev) => ({ kind: "event", event: ev }));

const op: fc.Arbitrary<Op> = fc.oneof(
  {
    weight: 4,
    arbitrary: event(
      fc
        .tuple(where, mods, fc.constantFrom(0, 0, 0, 0, 1, 2))
        .map(([p, m, button]) => ({ type: "pointer.down", ...p, ...m, button })),
    ),
  },
  {
    weight: 10,
    arbitrary: event(fc.tuple(where, mods).map(([p, m]) => ({ type: "pointer.move", ...p, ...m }))),
  },
  {
    weight: 4,
    arbitrary: event(fc.tuple(where, mods).map(([p, m]) => ({ type: "pointer.up", ...p, ...m }))),
  },
  { weight: 6, arbitrary: fc.constant<Op>({ kind: "event", event: { type: "frame" } }) },
  {
    weight: 2,
    arbitrary: event(
      fc.constantFrom<MachineEvent>(
        { type: "pointer.leave" },
        { type: "pointer.cancel" },
        { type: "blur" },
      ),
    ),
  },
  {
    weight: 2,
    arbitrary: event(
      fc
        .tuple(fc.constantFrom(...KEYS, ...MODIFIER_NAMES), mods, fc.boolean())
        .map(([key, m, up]) => ({ type: up ? "key.up" : "key.down", key, ...m })),
    ),
  },
  {
    weight: 1,
    arbitrary: event(fc.constantFrom(...ACTIONS).map((action) => ({ type: "action", action }))),
  },
  {
    weight: 2,
    arbitrary: event(
      fc
        .tuple(where, fc.constantFrom(VIDEO, AUDIO, IMAGE), fc.boolean())
        .map(([p, media, drop]) => ({ type: drop ? "media.drop" : "media.over", media, ...p })),
    ),
  },
  { weight: 1, arbitrary: fc.constant<Op>({ kind: "event", event: { type: "media.leave" } }) },
  {
    weight: 1,
    arbitrary: event(
      fc
        .oneof(
          fc.constantFrom(...TOOLS).map((tool) => ({ tool })),
          fc.boolean().map((snapping) => ({ snapping })),
          fc.boolean().map((linkedSelection) => ({ linkedSelection })),
          fc
            .constantFrom<"overwrite" | "insert">("overwrite", "insert")
            .map((dropOnClip) => ({ dropOnClip })),
          fc.subarray(MODEL.items.map((it) => it.id)).map((selection) => ({ selection })),
          fc
            .subarray(["Y", "Z", "B", "b"])
            .map((ids) => ({ heldBy: new Map(ids.map((id) => [id, "agent:claude"] as const)) })),
        )
        .map((env): MachineEvent => ({ type: "env", env })),
    ),
  },
  {
    weight: 3,
    arbitrary: fc
      .tuple(
        fc.constantFrom("ok", "invalid", "locked", "conflict", "not_found", "internal"),
        fc.boolean(),
      )
      .map(([code, clamped]): Op => ({ kind: "result", ok: code === "ok", code, clamped })),
  },
  { weight: 1, arbitrary: fc.boolean().map((stale): Op => ({ kind: "cancelled", stale })) },
  {
    weight: 1,
    arbitrary: fc
      .subarray(["Z", "Y", "B", "M", "C"], { maxLength: 2 })
      .map((drop): Op => ({ kind: "model", drop })),
  },
);

const ENDS_POINTER: ReadonlySet<string> = new Set(["pointer.up", "pointer.cancel", "blur"]);

function check(ops: readonly Op[]): void {
  const chart = readChart();
  const states = new Set<StateName>(MACHINE_STATES);
  const w = new World({ keyframes: true });
  const run = (e: MachineEvent): void => {
    const out = w.send(e);
    const name = w.state.mode.name;
    expect(states.has(name)).toBe(true);
    // A transaction exactly in the gesture states, and it is the engine's open one.
    const tx = "tx" in w.state.mode ? w.state.mode.tx : null;
    expect(tx !== null).toBe(GESTURE_STATES.has(name));
    expect(w.engine.open.size).toBe(tx === null ? 0 : 1);
    if (tx !== null) expect(w.engine.open.has(tx.gesture)).toBe(true);
    if (ENDS_POINTER.has(e.type)) expect(w.engine.open.size).toBe(0);
    // Coalescing: a frame sends at most one update; other events only flush one before a commit.
    const updates = out.intents.filter((i) => i.type === "update_tx").length;
    expect(updates).toBeLessThanOrEqual(1);
    if (e.type !== "frame") {
      out.intents.forEach((i, n) => {
        if (i.type === "update_tx") expect(out.intents[n + 1]?.type).toBe("commit_tx");
      });
    }
    for (const edge of out.edges) expect(chart.edges.has(edgeKey(edge))).toBe(true);
  };
  for (const o of ops) {
    switch (o.kind) {
      case "event":
        run(o.event);
        break;
      case "result": {
        const gesture = w.engine.current ?? w.state.lastGesture;
        run(
          o.ok
            ? { type: "engine.result", gesture, ok: true, clamped: o.clamped }
            : {
                type: "engine.result",
                gesture,
                ok: false,
                code: o.code,
                ...(o.code === "conflict" && { reason: "item_in_gesture", holder: "agent:claude" }),
              },
        );
        break;
      }
      case "cancelled": {
        const open = w.engine.current;
        const gesture = o.stale || open === null ? w.state.lastGesture + 7 : open;
        run({ type: "engine.cancelled", gesture, reason: "idle_timeout" });
        break;
      }
      case "model":
        w.setModel(without(MODEL, ...o.drop));
        expect(w.engine.open.size).toBe("tx" in w.state.mode ? 1 : 0);
        break;
    }
  }
  // Whatever the stream did, losing the window closes everything, and begins pair with closes.
  run({ type: "blur" });
  expect(w.engine.open.size).toBe(0);
  expect(w.engine.closes).toBe(w.engine.begins);
}

describe("random event streams", () => {
  it("keep the machine in its chart and every gesture transaction paired", () => {
    fc.assert(
      fc.property(fc.array(op, { minLength: 1, maxLength: 80 }), (ops) => check(ops)),
      { numRuns: 400 },
    );
  });

  it("reach every gesture state (the generator is not too timid)", () => {
    const seen = new Set<string>();
    fc.assert(
      fc.property(fc.array(op, { minLength: 20, maxLength: 80 }), (ops) => {
        const w = new World({ keyframes: true });
        for (const o of ops) {
          if (o.kind === "event") w.send(o.event);
          seen.add(w.state.mode.name);
        }
      }),
      { numRuns: 300, seed: 2408 },
    );
    for (const s of ["moving", "trimming", "selecting", "scrubbing", "dropping", "panning"]) {
      expect(seen.has(s)).toBe(true);
    }
  });
});
