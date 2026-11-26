// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.08: `createTimelineMachine` — coalescing (a drag sends at most one update per animation
// frame), frame requests only while something waits, listeners, re-entrant sends and dispose.
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath } from "@cutweave/protocol/wasm";
import { beforeAll, describe, expect, it } from "vitest";
import { machineKeyAction } from "./keys.ts";
import { createTimelineMachine, type TimelineMachine } from "./machine.ts";
import { clip, type Pos } from "./testing/transcript.ts";
import { MODEL, PX_PER_SEC, World } from "./testing/world.ts";
import type { FrameScheduler, MachineIntent } from "./types.ts";

beforeAll(async () => {
  await initMath(mathModuleBytes());
}, 900_000);

/** A frame scheduler the test runs by hand. */
function manualFrames(): FrameScheduler & {
  pending: (() => void)[];
  requested: number;
  tick(): void;
} {
  const s = {
    pending: [] as (() => void)[],
    requested: 0,
    request(cb: () => void) {
      s.requested++;
      s.pending.push(cb);
      return cb;
    },
    cancel(handle: unknown) {
      s.pending = s.pending.filter((cb) => cb !== handle);
    },
    tick() {
      const due = s.pending;
      s.pending = [];
      for (const cb of due) cb();
    },
  };
  return s;
}

interface Rig {
  readonly machine: TimelineMachine;
  readonly frames: ReturnType<typeof manualFrames>;
  readonly intents: MachineIntent[];
  readonly at: (pos: Pos) => { x: number; y: number };
}

function rig(): Rig {
  const world = new World();
  const frames = manualFrames();
  const machine = createTimelineMachine(
    { ...world.deps, frames },
    { model: MODEL, sequence: "S1", pxPerSec: PX_PER_SEC },
  );
  const intents: MachineIntent[] = [];
  machine.subscribe((batch) => intents.push(...batch));
  return { machine, frames, intents, at: (pos) => pos(world) };
}

const updates = (intents: readonly MachineIntent[]): number =>
  intents.filter((i) => i.type === "update_tx").length;

describe("createTimelineMachine", () => {
  it("sends at most one update per animation frame however many pointer events arrive", () => {
    const { machine, frames, intents, at } = rig();
    const z = at(clip("Z"));
    machine.send({ type: "pointer.down", ...z, button: 0 });
    machine.send({ type: "pointer.move", x: z.x + 4, y: z.y });
    expect(intents.filter((i) => i.type === "begin_tx")).toEqual([
      { type: "begin_tx", gesture: 1, edit: "move" },
    ]);
    // 100 pointer events within one frame: one frame asked for, nothing sent yet.
    for (let i = 1; i <= 100; i++) machine.send({ type: "pointer.move", x: z.x + 4 + i, y: z.y });
    expect(frames.requested).toBe(1);
    expect(updates(intents)).toBe(0);
    frames.tick();
    expect(updates(intents)).toBe(1);
    // 10 frames of 50 events each: at most one update per frame.
    for (let f = 0; f < 10; f++) {
      for (let i = 0; i < 50; i++)
        machine.send({ type: "pointer.move", x: z.x + 110 + f * 50 + i, y: z.y });
      frames.tick();
    }
    expect(updates(intents)).toBeLessThanOrEqual(11);
    expect(updates(intents)).toBeGreaterThanOrEqual(10);
    machine.send({ type: "pointer.up", x: z.x + 700, y: z.y });
    expect(intents.filter((i) => i.type === "commit_tx")).toEqual([
      { type: "commit_tx", gesture: 1 },
    ]);
    // Nothing waits any more: no frame is asked for.
    const asked = frames.requested;
    machine.send({ type: "pointer.move", x: 300, y: 300 });
    frames.tick();
    expect(frames.requested).toBe(asked);
  });

  it("asks for no frame while hovering and none for a position already sent", () => {
    const { machine, frames, intents, at } = rig();
    const z = at(clip("Z"));
    for (let i = 0; i < 20; i++) machine.send({ type: "pointer.move", x: z.x + i, y: z.y });
    expect(frames.requested).toBe(0);
    machine.send({ type: "pointer.down", ...z, button: 0 });
    machine.send({ type: "pointer.move", x: z.x + 40, y: z.y });
    frames.tick();
    expect(updates(intents)).toBe(1);
    const asked = frames.requested;
    // The same position again changes nothing: no frame, no update.
    machine.send({ type: "pointer.move", x: z.x + 40, y: z.y });
    expect(frames.requested).toBe(asked);
  });

  it("runs events sent from a listener after the current one, in order", () => {
    const { machine, intents, at } = rig();
    const seen: string[] = [];
    machine.subscribe((batch) => {
      for (const i of batch) {
        seen.push(i.type);
        // The host reacts to the selection by sending the view state back (like viewStore).
        if (i.type === "set_selection") machine.send({ type: "env", env: { selection: i.ids } });
      }
    });
    const z = at(clip("Z"));
    machine.send({ type: "pointer.down", ...z, button: 0 });
    machine.send({ type: "pointer.up", ...z, button: 0 });
    expect(seen[0]).toBe("set_selection");
    expect(machine.state.env.selection).toEqual(["Z"]);
    expect(machine.state.mode.name).toBe("hover");
    expect(intents.filter((i) => i.type === "set_selection")).toHaveLength(1);
  });

  it("dispose rolls back an open gesture and stops its frame", () => {
    const { machine, frames, intents, at } = rig();
    const edges: string[] = [];
    machine.onEdge((e) => edges.push(`${e.from}>${e.to}`));
    const z = at(clip("Z"));
    machine.send({ type: "pointer.down", ...z, button: 0 });
    machine.send({ type: "pointer.move", x: z.x + 40, y: z.y });
    expect(frames.pending).toHaveLength(1);
    machine.dispose();
    expect(intents.filter((i) => i.type === "cancel_tx")).toEqual([
      { type: "cancel_tx", gesture: 1, reason: "blur" },
    ]);
    expect(frames.pending).toHaveLength(0);
    expect(edges).toEqual(["idle>hover", "hover>pressed", "pressed>moving", "moving>idle"]);
    // A disposed machine ignores events.
    machine.send({ type: "pointer.down", ...z, button: 0 });
    expect(machine.state.mode.name).toBe("idle");
  });

  it("unsubscribed listeners hear nothing more", () => {
    const { machine, at } = rig();
    const heard: number[] = [];
    const off = machine.subscribe((batch) => heard.push(batch.length));
    const z = at(clip("Z"));
    machine.send({ type: "pointer.move", ...z });
    off();
    machine.send({ type: "pointer.move", x: 300, y: 300 });
    expect(heard).toHaveLength(1);
  });
});

describe("machineKeyAction", () => {
  it("maps the default keys, either case, and nothing with Ctrl, Cmd or Alt", () => {
    expect(machineKeyAction("S")).toBe("split");
    expect(machineKeyAction("Delete", { shift: true })).toBe("ripple_delete");
    expect(machineKeyAction(">", { shift: true })).toBe("nudge_right_10");
    expect(machineKeyAction("c")).toBe("tool.razor");
    expect(machineKeyAction("s", { ctrl: true })).toBeNull();
    expect(machineKeyAction("s", { meta: true })).toBeNull();
    expect(machineKeyAction("Escape", { ctrl: true })).toBe("cancel");
    expect(machineKeyAction("q")).toBeNull();
  });
});
