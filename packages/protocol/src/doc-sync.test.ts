// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { applyDocEvent, DocMirror, docFromSnapshot, EMPTY_DOC } from "./doc-mirror.ts";
import { DocSync } from "./doc-sync.ts";
import type { DocPatched, PatchOp, Project } from "./generated/types.ts";
import { MockTransport, type ProjectSnapshot, RpcClient } from "./rpc.ts";

/** A tiny document: only `meta.name` matters to these tests. */
const project = (name: string): Project =>
  ({ meta: { name }, settings: {}, media: {}, sequences: {}, custom: {} }) as unknown as Project;

const rename = (from: string, to: string): PatchOp[] => [
  { op: "replace", path: "/meta/name", old: from, value: to } as PatchOp,
];

function patched(rev: number, prev: number, ops: PatchOp[]): DocPatched {
  return {
    rev,
    prev_rev: prev,
    committed_rev: rev,
    actor: "user",
    tx_id: `tx_${rev}`,
    cause: "command",
    request_id: null,
    ops,
  } as unknown as DocPatched;
}

function setup(snapshot: () => ProjectSnapshot) {
  const transport = new MockTransport().handle("project.snapshot", snapshot);
  const rpc = new RpcClient(transport);
  const mirror = new DocMirror();
  const sync = new DocSync(rpc, mirror);
  const snapshots = () => transport.requests.filter((r) => r.method === "project.snapshot").length;
  return { transport, mirror, sync, snapshots };
}

const wait = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("applyDocEvent", () => {
  const ready = docFromSnapshot({ rev: 4, committed_rev: 4, project: project("a") });

  it("applies the next revision", () => {
    const out = applyDocEvent(ready, patched(5, 4, rename("a", "b")));
    expect(out.result).toBe("applied");
    expect(out.state.rev).toBe(5);
    expect(out.state.project?.meta.name).toBe("b");
    expect(ready.project?.meta.name).toBe("a"); // the old state is untouched
  });

  it("drops an event the snapshot already contains", () => {
    expect(applyDocEvent(ready, patched(4, 3, rename("x", "y"))).result).toBe("stale");
    expect(applyDocEvent(ready, patched(2, 1, rename("x", "y"))).result).toBe("stale");
  });

  it("reports a gap when prev_rev does not continue the mirror", () => {
    expect(applyDocEvent(ready, patched(9, 8, rename("a", "b"))).result).toBe("gap");
  });

  it("reports a gap when the ops do not apply (old value differs)", () => {
    const out = applyDocEvent(ready, patched(5, 4, rename("not-a", "b")));
    expect(out.result).toBe("gap");
    expect(out.state).toBe(ready);
  });

  it("an empty mirror needs a snapshot first", () => {
    expect(applyDocEvent(EMPTY_DOC, patched(1, 0, [])).result).toBe("gap");
  });
});

describe("DocSync", () => {
  it("subscribes before it reads the snapshot", async () => {
    const order: string[] = [];
    const transport = new MockTransport().handle("project.snapshot", () => {
      order.push("snapshot");
      return { rev: 1, committed_rev: 1, project: project("a") };
    });
    transport.settled = () => {
      order.push("settled");
      return Promise.resolve();
    };
    const sync = new DocSync(new RpcClient(transport), new DocMirror());
    await sync.start();
    expect(order).toEqual(["settled", "snapshot"]);
  });

  it("applies events in order after the snapshot", async () => {
    const { transport, mirror, sync, snapshots } = setup(() => ({
      rev: 4,
      committed_rev: 4,
      project: project("a"),
    }));
    await sync.start();
    transport.emit("doc.patched", patched(5, 4, rename("a", "b")));
    transport.emit("doc.patched", patched(6, 5, rename("b", "c")));
    expect(mirror.state.rev).toBe(6);
    expect(mirror.state.project?.meta.name).toBe("c");
    expect(snapshots()).toBe(1);
  });

  it("a forced gap triggers exactly one project.snapshot", async () => {
    let live = { rev: 4, project: project("a") };
    const { transport, mirror, sync, snapshots } = setup(() => ({
      rev: live.rev,
      committed_rev: live.rev,
      project: live.project,
    }));
    await sync.start();
    expect(snapshots()).toBe(1);

    // Revisions 5..8 were missed: the event for 9 continues from 8, not from 4.
    live = { rev: 9, project: project("nine") };
    transport.emit("doc.patched", patched(9, 8, rename("a", "nine")));
    await wait();
    expect(snapshots()).toBe(2);
    expect(mirror.state.rev).toBe(9);
    expect(mirror.state.project?.meta.name).toBe("nine");

    // And the stream carries on from there without another snapshot.
    transport.emit("doc.patched", patched(10, 9, rename("nine", "ten")));
    await wait();
    expect(snapshots()).toBe(2);
    expect(mirror.state.rev).toBe(10);
  });

  it("several gap events in a burst share one snapshot", async () => {
    let live = { rev: 4, project: project("a") };
    const { transport, sync, snapshots } = setup(() => ({
      rev: live.rev,
      committed_rev: live.rev,
      project: live.project,
    }));
    await sync.start();
    live = { rev: 12, project: project("x") };
    transport.emit("doc.patched", patched(9, 8, rename("a", "b")));
    transport.emit("doc.patched", patched(10, 9, rename("b", "c")));
    transport.emit("doc.patched", patched(11, 10, rename("c", "d")));
    await wait();
    expect(snapshots()).toBe(2);
  });

  it("holds events that arrive during the snapshot and drops those the snapshot contains", async () => {
    let release: (() => void) | undefined;
    const transport = new MockTransport();
    transport.handle("project.snapshot", () => ({
      rev: 6,
      committed_rev: 6,
      project: project("six"),
    }));
    // Delay the snapshot answer so events can overtake it.
    const original = transport.request.bind(transport);
    transport.request = async (method, params, meta) => {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return original(method, params, meta);
    };
    const mirror = new DocMirror();
    const sync = new DocSync(new RpcClient(transport), mirror);
    const started = sync.start();
    await wait();
    transport.emit("doc.patched", patched(5, 4, rename("a", "five"))); // already in the snapshot
    transport.emit("doc.patched", patched(7, 6, rename("six", "seven"))); // newer
    release?.();
    await started;
    expect(mirror.state.rev).toBe(7);
    expect(mirror.state.project?.meta.name).toBe("seven");
  });

  it("doc.reset fetches a new snapshot", async () => {
    let live = { rev: 4, project: project("a") };
    const { transport, mirror, sync, snapshots } = setup(() => ({
      rev: live.rev,
      committed_rev: live.rev,
      project: live.project,
    }));
    await sync.start();
    live = { rev: 1, project: project("another project") };
    transport.emit("doc.reset", { rev: 1, prev_rev: 4, committed_rev: 1 });
    await wait();
    expect(snapshots()).toBe(2);
    expect(mirror.state.project?.meta.name).toBe("another project");
  });

  it("stop() ends the subscriptions", async () => {
    const { transport, mirror, sync } = setup(() => ({
      rev: 1,
      committed_rev: 1,
      project: project("a"),
    }));
    await sync.start();
    sync.stop();
    transport.emit("doc.patched", patched(2, 1, rename("a", "b")));
    expect(mirror.state.rev).toBe(1);
  });
});
