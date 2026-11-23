// SPDX-License-Identifier: GPL-3.0-or-later
// GestureSession against a scripted transport: transaction lifecycle, frame throttling, one
// request in flight, and what each kind of engine refusal does to the gesture.
import { MockTransport, type RecordedRequest, RpcClient, RpcError } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { type FrameScheduler, GestureSession, type GestureUpdate } from "./gesture.ts";

const item = "itm_1" as never;

/** Frames that fire only when the test says so. */
function manualFrames(): FrameScheduler & { tick(): void; readonly pending: number } {
  let queue: (() => void)[] = [];
  return {
    request(cb) {
      queue.push(cb);
      return cb;
    },
    cancel(handle) {
      queue = queue.filter((cb) => cb !== handle);
    },
    tick() {
      const run = queue;
      queue = [];
      for (const cb of run) cb();
    },
    get pending() {
      return queue.length;
    },
  };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function engine(handlers: Record<string, (params: unknown) => unknown> = {}) {
  const transport = new MockTransport()
    .handle("edit.begin_tx", () => ({ tx: "tx_1", kind: "gesture", label: "Move clip", rev: 1 }))
    .handle("edit.commit_tx", () => ({
      tx: "tx_1",
      kind: "gesture",
      changed: true,
      rev: 5,
      committed_rev: 5,
    }))
    .handle("edit.cancel_tx", () => ({ tx: "tx_1", kind: "gesture", rev: 6 }))
    .handle("item.move", () => ({ items: [], created_tracks: [] }))
    .handle("clip.trim", () => ({ items: [] }));
  for (const [name, fn] of Object.entries(handlers)) transport.handle(name, fn);
  return { transport, rpc: new RpcClient(transport) };
}

const move = (start: number): GestureUpdate => ({ name: "item.move", input: { item, start } });
const methods = (requests: readonly RecordedRequest[]) => requests.map((r) => r.method);

describe("GestureSession", () => {
  it("opens a gesture transaction labelled for the undo history", async () => {
    const { transport, rpc } = engine();
    const session = new GestureSession({ rpc, label: "Move clip", frames: manualFrames() });
    await session.finish(true);
    expect(transport.requests[0]).toEqual({
      method: "edit.begin_tx",
      params: { kind: "gesture", label: "Move clip" },
    });
  });

  it("sends at most one update per frame, the newest, inside the transaction", async () => {
    const { transport, rpc } = engine();
    const frames = manualFrames();
    const session = new GestureSession({ rpc, label: "Move clip", frames });
    for (let i = 1; i <= 10; i++) session.update(move(i));
    expect(frames.pending).toBe(1);
    frames.tick();
    await flush();
    expect(session.updates).toBe(1);
    const sent = transport.requests.filter((r) => r.method === "item.move");
    expect(sent).toEqual([
      { method: "item.move", params: { item, start: 10 }, meta: { tx: "tx_1" } },
    ]);
  });

  it("keeps one request in flight and sends the newest position when it returns", async () => {
    let release: () => void = () => {};
    const { transport, rpc } = engine({
      "item.move": () =>
        new Promise((resolve) => {
          release = () => resolve({ items: [], created_tracks: [] });
        }),
    });
    const frames = manualFrames();
    const session = new GestureSession({ rpc, label: "Move clip", frames });
    session.update(move(1));
    frames.tick();
    await flush();
    session.update(move(2));
    session.update(move(3));
    frames.tick(); // nothing scheduled while a request is in flight
    await flush();
    expect(session.updates).toBe(1);
    release();
    await flush();
    frames.tick();
    await flush();
    expect(session.updates).toBe(2);
    expect(transport.requests.filter((r) => r.method === "item.move").at(-1)?.params).toEqual({
      item,
      start: 3,
    });
  });

  it("does not resend a position that was already sent", async () => {
    const { rpc } = engine();
    const frames = manualFrames();
    const session = new GestureSession({ rpc, label: "Move clip", frames });
    session.update(move(1));
    frames.tick();
    await flush();
    session.update(move(1));
    frames.tick();
    await flush();
    expect(session.updates).toBe(1);
    await session.finish(true);
    expect(session.updates).toBe(1);
  });

  it("flushes the newest position on release, then commits", async () => {
    const { transport, rpc } = engine();
    const session = new GestureSession({ rpc, label: "Move clip", frames: manualFrames() });
    session.update(move(7));
    const outcome = await session.finish(true);
    expect(outcome).toEqual({ kind: "committed", changed: true });
    expect(methods(transport.requests)).toEqual(["edit.begin_tx", "item.move", "edit.commit_tx"]);
  });

  it("rolls back on cancel without sending pending positions", async () => {
    const { transport, rpc } = engine();
    const session = new GestureSession({ rpc, label: "Move clip", frames: manualFrames() });
    session.update(move(7));
    expect(await session.finish(false)).toEqual({ kind: "cancelled" });
    expect(methods(transport.requests)).toEqual(["edit.begin_tx", "edit.cancel_tx"]);
  });

  it("stays open through an overlap refusal and commits once a later position is accepted", async () => {
    const { transport, rpc } = engine({
      "item.move": (p) => {
        if ((p as { start: number }).start === 2) throw new RpcError("invalid", "would overlap");
        return { items: [], created_tracks: [] };
      },
    });
    const frames = manualFrames();
    const onAbort = vi.fn();
    const session = new GestureSession({ rpc, label: "Move clip", frames, onAbort });
    session.update(move(2));
    frames.tick();
    await flush();
    session.update(move(3));
    expect(await session.finish(true)).toEqual({ kind: "committed", changed: true });
    expect(onAbort).not.toHaveBeenCalled();
    expect(methods(transport.requests).at(-1)).toBe("edit.commit_tx");
  });

  it("reverts when the engine refuses the final position", async () => {
    const { transport, rpc } = engine({
      "item.move": () => {
        throw new RpcError("invalid", "item would overlap item");
      },
    });
    const session = new GestureSession({ rpc, label: "Move clip", frames: manualFrames() });
    session.update(move(2));
    const outcome = await session.finish(true);
    expect(outcome.kind).toBe("refused");
    expect(methods(transport.requests)).toEqual(["edit.begin_tx", "item.move", "edit.cancel_tx"]);
  });

  it("reverts a locked track's gesture on release", async () => {
    const { rpc } = engine({
      "clip.trim": () => {
        throw new RpcError("locked", "track is locked");
      },
    });
    const session = new GestureSession({ rpc, label: "Trim clip", frames: manualFrames() });
    session.update({ name: "clip.trim", input: { item, edge: "end", to: 5 } });
    const outcome = await session.finish(true);
    expect(outcome.kind === "refused" && outcome.error.code).toBe("locked");
  });

  it("aborts at once on another actor's gesture lock, rolls back and reports it", async () => {
    const { transport, rpc } = engine({
      "item.move": () => {
        throw new RpcError("conflict", "in gesture", undefined, { reason: "item_in_gesture" });
      },
    });
    const frames = manualFrames();
    const onAbort = vi.fn();
    const session = new GestureSession({ rpc, label: "Move clip", frames, onAbort });
    session.update(move(2));
    frames.tick();
    await flush();
    await flush();
    expect(onAbort).toHaveBeenCalledTimes(1);
    expect(methods(transport.requests)).toContain("edit.cancel_tx");
    session.update(move(3)); // ignored after the abort
    const outcome = await session.finish(true);
    expect(outcome).toMatchObject({ kind: "aborted", reported: true });
    expect(methods(transport.requests)).not.toContain("edit.commit_tx");
  });

  it("reports a transaction that could not open", async () => {
    const { transport, rpc } = engine({
      "edit.begin_tx": () => {
        throw new RpcError("invalid", "this connection already has an open transaction");
      },
    });
    const onAbort = vi.fn();
    const session = new GestureSession({
      rpc,
      label: "Move clip",
      frames: manualFrames(),
      onAbort,
    });
    await flush();
    expect(onAbort).toHaveBeenCalledTimes(1);
    session.update(move(1));
    expect(await session.finish(true)).toMatchObject({ kind: "aborted", reported: true });
    expect(methods(transport.requests)).toEqual(["edit.begin_tx"]);
  });

  it("opens only after the work it must follow", async () => {
    const { transport, rpc } = engine();
    let done: () => void = () => {};
    const after = new Promise<void>((resolve) => {
      done = resolve;
    });
    const session = new GestureSession({ rpc, label: "Move clip", frames: manualFrames(), after });
    await flush();
    expect(transport.requests).toEqual([]);
    done();
    await session.finish(true);
    expect(methods(transport.requests)).toEqual(["edit.begin_tx", "edit.commit_tx"]);
  });
});
