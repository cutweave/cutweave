// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.07: a drag in the graph editor is one gesture transaction and one undo step. Every step
// starts with `_meta.replace` (the engine rolls the earlier steps back), only the newest
// position is sent, a refused position falls back to the last accepted one, and a soft-lock
// conflict ends the gesture with one message.
import { type RpcClient, RpcError } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { GraphGesture, type GraphStep } from "./graphGesture.ts";

interface Sent {
  readonly name: string;
  readonly args: unknown;
  readonly meta: unknown;
}

function rig(fail: (call: Sent) => Error | null = () => null) {
  const sent: Sent[] = [];
  const rpc = {
    call(name: string, args: unknown, meta?: unknown): Promise<unknown> {
      const call = { name, args, meta };
      sent.push(call);
      const error = fail(call);
      if (error !== null) return Promise.reject(error);
      if (name === "edit.begin_tx") return Promise.resolve({ tx: "tx_1" });
      if (name === "edit.commit_tx") return Promise.resolve({ changed: true });
      return Promise.resolve({ echoed: args });
    },
  } as unknown as RpcClient;
  const queue: (() => void)[] = [];
  const frames = {
    request: (cb: () => void) => queue.push(cb),
    cancel: () => {
      queue.length = 0;
    },
  };
  /** Runs the waiting frame and lets the engine answer. */
  const frame = async (): Promise<void> => {
    for (const cb of queue.splice(0)) cb();
    for (let i = 0; i < 12; i++) await Promise.resolve();
  };
  const names = (): string[] => sent.map((s) => s.name);
  return { sent, rpc, frames, frame, names };
}

/** A step of two commands: a move and a value. */
const step = (n: number): GraphStep => ({
  key: `s${n}`,
  run: async (send) => {
    await send("keyframe.move", { n });
    await send("keyframe.set", { n });
    return n;
  },
});

describe("a graph editor gesture", () => {
  it("opens one transaction, replaces with each step and commits once", async () => {
    const r = rig();
    const applied = vi.fn();
    const g = new GraphGesture({
      rpc: r.rpc,
      label: "Move keyframe",
      frames: r.frames,
      onApplied: applied,
    });
    g.update(step(1));
    await r.frame();
    g.update(step(2));
    await r.frame();
    const outcome = await g.finish(true);
    expect(outcome).toEqual({ kind: "committed", output: 2 });
    expect(r.sent).toEqual([
      { name: "edit.begin_tx", args: { kind: "gesture", label: "Move keyframe" }, meta: undefined },
      // The first command of a step rolls the earlier steps back; the rest add to it.
      { name: "keyframe.move", args: { n: 1 }, meta: { tx: "tx_1", replace: true } },
      { name: "keyframe.set", args: { n: 1 }, meta: { tx: "tx_1" } },
      { name: "keyframe.move", args: { n: 2 }, meta: { tx: "tx_1", replace: true } },
      { name: "keyframe.set", args: { n: 2 }, meta: { tx: "tx_1" } },
      { name: "edit.commit_tx", args: { tx: "tx_1" }, meta: undefined },
    ]);
    expect(applied.mock.calls.map((c) => c[0])).toEqual([1, 2]);
    expect(g.steps).toBe(2);
  });

  it("sends only the newest position: many pointer moves in a frame are one step", async () => {
    const r = rig();
    const g = new GraphGesture({ rpc: r.rpc, label: "Move keyframe", frames: r.frames });
    for (let n = 1; n <= 50; n++) g.update(step(n));
    await r.frame();
    g.update(step(50)); // the same position again: nothing to send
    await r.frame();
    await g.finish(true);
    expect(g.steps).toBe(1);
    expect(r.sent.filter((s) => s.name === "keyframe.move").map((s) => s.args)).toEqual([
      { n: 50 },
    ]);
  });

  it("sends the last position on release even when no frame came in between", async () => {
    const r = rig();
    const g = new GraphGesture({ rpc: r.rpc, label: "Move keyframe", frames: r.frames });
    g.update(step(7));
    const outcome = await g.finish(true);
    expect(outcome).toEqual({ kind: "committed", output: 7 });
    expect(r.names()).toEqual(["edit.begin_tx", "keyframe.move", "keyframe.set", "edit.commit_tx"]);
  });

  it("rolls back on cancel: no undo step", async () => {
    const r = rig();
    const g = new GraphGesture({ rpc: r.rpc, label: "Move keyframe", frames: r.frames });
    g.update(step(1));
    await r.frame();
    expect(await g.finish(false)).toEqual({ kind: "cancelled" });
    expect(r.names().slice(-1)).toEqual(["edit.cancel_tx"]);
    expect(r.names()).not.toContain("edit.commit_tx");
    // Ending twice is the same ending.
    expect(await g.finish(true)).toEqual({ kind: "cancelled" });
  });

  it("falls back to the last accepted position when the newest one is refused", async () => {
    // The value of position 2 is out of range: its move was applied, its set refused.
    const r = rig((call) =>
      call.name === "keyframe.set" && (call.args as { n: number }).n === 2
        ? new RpcError("invalid", "value out of range")
        : null,
    );
    const g = new GraphGesture({ rpc: r.rpc, label: "Move keyframe", frames: r.frames });
    g.update(step(1));
    await r.frame();
    g.update(step(2));
    await r.frame();
    const outcome = await g.finish(true);
    expect(outcome).toEqual({ kind: "committed", output: 1 });
    // Position 1 again, whole (its first command replaces the half-applied position 2).
    expect(r.sent.slice(-3)).toEqual([
      { name: "keyframe.move", args: { n: 1 }, meta: { tx: "tx_1", replace: true } },
      { name: "keyframe.set", args: { n: 1 }, meta: { tx: "tx_1" } },
      { name: "edit.commit_tx", args: { tx: "tx_1" }, meta: undefined },
    ]);
  });

  it("is refused, with nothing kept, when the engine took no position at all", async () => {
    const r = rig((call) =>
      call.name === "keyframe.move" ? new RpcError("invalid", "a key is already there") : null,
    );
    const g = new GraphGesture({ rpc: r.rpc, label: "Move keyframe", frames: r.frames });
    g.update(step(1));
    await r.frame();
    const outcome = await g.finish(true);
    expect(outcome.kind).toBe("refused");
    expect(r.names().slice(-1)).toEqual(["edit.cancel_tx"]);
    expect(r.names()).not.toContain("edit.commit_tx");
  });

  it("ends at once on a soft-lock conflict: rolled back, told once", async () => {
    const conflict = new RpcError("conflict", "in use", undefined, {
      reason: "item_in_gesture",
      holder: "agent:claude",
    });
    const r = rig((call) => (call.name === "keyframe.move" ? conflict : null));
    const aborted = vi.fn();
    const g = new GraphGesture({
      rpc: r.rpc,
      label: "Move keyframe",
      frames: r.frames,
      onAbort: aborted,
    });
    g.update(step(1));
    await r.frame();
    expect(aborted).toHaveBeenCalledTimes(1);
    expect(aborted.mock.calls[0]?.[0]).toBe(conflict);
    g.update(step(2)); // ignored: the gesture is over
    await r.frame();
    const outcome = await g.finish(true);
    expect(outcome).toEqual({ kind: "aborted", error: conflict, reported: true });
    expect(r.names().filter((n) => n === "keyframe.move")).toHaveLength(1);
    expect(r.names()).toContain("edit.cancel_tx");
    expect(r.names()).not.toContain("edit.commit_tx");
  });

  it("waits for the panel's earlier engine work before opening", async () => {
    const r = rig();
    let release: () => void = () => undefined;
    const before = new Promise<void>((resolve) => {
      release = resolve;
    });
    const g = new GraphGesture({
      rpc: r.rpc,
      label: "Move keyframe",
      frames: r.frames,
      after: before,
    });
    g.update(step(1));
    await r.frame();
    expect(r.names()).toEqual([]);
    release();
    await g.finish(true);
    expect(r.names()[0]).toBe("edit.begin_tx");
  });
});
