// SPDX-License-Identifier: GPL-3.0-or-later
// Issue #222: a marker dragged on the ruler against a scripted engine. A click goes to the marker,
// a drag is one gesture transaction of `timeline.move_marker` (one undo step), snapped by the
// timeline's snapping, and Esc rolls it back.
import { MockTransport, RpcClient, RpcError } from "@cutweave/protocol";
import type {
  HitResult,
  SnapGuide,
  TimelineMarker,
  TimelineModel,
  TimelinePointerEvent,
} from "@cutweave/timeline";
import { describe, expect, it, vi } from "vitest";
import { MarkerDrag } from "./markerDrag.ts";

const F = 705_600_000;
const FRAME = F / 30;
const MARKER: TimelineMarker = { id: "mrk_1", t: 2 * F, label: "Intro", kind: "user" };
const OTHER: TimelineMarker = { id: "mrk_2", t: 5 * F, label: "", kind: "user" };
const MODEL: TimelineModel = {
  fps: { num: 30, den: 1 },
  tracks: [],
  items: [],
  markers: [MARKER, OTHER],
};

const hit = (zone: HitResult["zone"]): HitResult => ({
  zone,
  time: 0,
  trackId: null,
  trackIndex: -1,
  itemId: null,
  itemIndex: -1,
  keyframeIndex: -1,
  keyframeTime: null,
  prop: null,
  toggle: null,
});

const pointer = (
  type: TimelinePointerEvent["type"],
  x: number,
  extra: Partial<TimelinePointerEvent> = {},
): TimelinePointerEvent => ({
  type,
  x,
  y: 20,
  hit: hit("ruler"),
  button: 0,
  buttons: 1,
  shiftKey: false,
  ctrlKey: false,
  altKey: false,
  metaKey: false,
  original: {} as MouseEvent,
  ...extra,
});

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function setup(handlers: Record<string, (params: unknown) => unknown> = {}) {
  const transport = new MockTransport()
    .handle("edit.begin_tx", () => ({ tx: "tx_1", kind: "gesture", label: "Move marker", rev: 1 }))
    .handle("edit.commit_tx", () => ({
      tx: "tx_1",
      kind: "gesture",
      changed: true,
      rev: 3,
      committed_rev: 3,
    }))
    .handle("edit.cancel_tx", () => ({ tx: "tx_1", kind: "gesture", rev: 4 }))
    .handle("timeline.move_marker", (params) => ({
      marker: MARKER.id,
      t: (params as { t: number }).t,
      previous: MARKER.t,
    }));
  for (const [name, fn] of Object.entries(handlers)) transport.handle(name, fn);
  const rpc = new RpcClient(transport);
  const seek = vi.fn();
  const refused = vi.fn();
  const guides = vi.fn<(g: readonly SnapGuide[]) => void>();
  const snapped: { model: TimelineModel; raw: number; suspended: boolean }[] = [];
  let busy = false;
  // Frames run at once: every pointer move may send.
  const frames = {
    request: (cb: () => void) => {
      cb();
      return cb;
    },
    cancel: () => undefined,
  };
  const drag = new MarkerDrag({
    rpc: () => rpc,
    label: () => "Move marker",
    // The marker's pin is 8 px around x = 200.
    markerAt: (x) => (Math.abs(x - 200) <= 4 ? MARKER : null),
    model: () => MODEL,
    // 100 px per second.
    flicksPerPx: () => F / 100,
    snap: (model, raw, suspended) => {
      // The shared snap index takes whole flicks only.
      if (!Number.isInteger(raw)) throw new RangeError(`${raw} is not an integer`);
      snapped.push({ model, raw, suspended });
      // Within 10 px of the other marker lands on it; else the frame grid.
      if (!suspended && Math.abs(raw - OTHER.t) <= F / 10) {
        return { t: OTHER.t, target: { kind: "marker", t: OTHER.t } };
      }
      return { t: Math.round(raw / FRAME) * FRAME, target: null };
    },
    guides,
    seek,
    refused,
    busy: () => busy,
    frames,
  });
  const sent = (method: string) => transport.requests.filter((r) => r.method === method);
  return {
    drag,
    transport,
    seek,
    refused,
    guides,
    snapped,
    sent,
    setBusy: (on: boolean) => (busy = on),
  };
}

describe("MarkerDrag", () => {
  it("leaves presses that are not on a marker to the timeline", () => {
    const { drag, setBusy } = setup();
    expect(drag.handle(pointer("down", 500))).toBe(false);
    expect(drag.handle(pointer("down", 200, { hit: hit("lane") }))).toBe(false);
    expect(drag.handle(pointer("down", 200, { hit: hit("header") }))).toBe(false);
    expect(drag.handle(pointer("down", 200, { button: 2 }))).toBe(false);
    expect(drag.handle(pointer("move", 200))).toBe(false);
    setBusy(true);
    expect(drag.handle(pointer("down", 200))).toBe(false);
    expect(drag.active).toBe(false);
  });

  it("takes a press on a pin under the playhead too (a marker just added with M)", () => {
    const { drag } = setup();
    expect(drag.handle(pointer("down", 200, { hit: hit("playhead") }))).toBe(true);
    expect(drag.active).toBe(true);
  });

  it("goes to the marker on a click, without an edit", async () => {
    const { drag, seek, transport } = setup();
    expect(drag.handle(pointer("down", 200))).toBe(true);
    expect(drag.handle(pointer("move", 201))).toBe(true); // under the drag threshold
    expect(drag.handle(pointer("up", 201))).toBe(true);
    await drag.settled();
    expect(seek).toHaveBeenCalledWith(MARKER.t);
    expect(transport.requests).toEqual([]);
    expect(drag.active).toBe(false);
  });

  it("moves the marker with the pointer in one gesture transaction, on frames", async () => {
    const { drag, sent, transport, seek, snapped } = setup();
    drag.handle(pointer("down", 200));
    drag.handle(pointer("move", 250)); // +0.5 s
    await flush();
    drag.handle(pointer("move", 301.4)); // +1.014 s: lands on a frame
    await flush();
    expect(drag.dragging).toBe(true);
    drag.handle(pointer("up", 301.4));
    await drag.settled();
    expect(transport.requests.map((r) => r.method)).toEqual([
      "edit.begin_tx",
      "timeline.move_marker",
      "timeline.move_marker",
      "edit.commit_tx",
    ]);
    expect(transport.requests[0]?.params).toEqual({ kind: "gesture", label: "Move marker" });
    expect(sent("timeline.move_marker").map((r) => r.params)).toEqual([
      { marker: MARKER.id, t: 2.5 * F },
      { marker: MARKER.id, t: Math.round(Math.round(3.014 * F) / FRAME) * FRAME },
    ]);
    expect(sent("timeline.move_marker").every((r) => r.meta?.tx === "tx_1")).toBe(true);
    expect(seek).not.toHaveBeenCalled();
    // The dragged marker is not a snap candidate of its own drag.
    expect(snapped[0]?.model.markers?.map((m) => m.id)).toEqual([OTHER.id]);
    expect(snapped.every((q) => q.model === snapped[0]?.model)).toBe(true);
  });

  it("snaps to what the timeline snaps to and shows the guide; Ctrl leaves the frame grid", async () => {
    const { drag, sent, guides } = setup();
    drag.handle(pointer("down", 200));
    drag.handle(pointer("move", 495)); // 5 px short of the other marker
    await flush();
    expect(guides).toHaveBeenLastCalledWith([{ kind: "marker", t: OTHER.t }]);
    drag.handle(pointer("move", 495, { ctrlKey: true }));
    await flush();
    expect(guides).toHaveBeenLastCalledWith([]);
    drag.handle(pointer("up", 495, { ctrlKey: true }));
    await drag.settled();
    expect(sent("timeline.move_marker").map((r) => (r.params as { t: number }).t)).toEqual([
      OTHER.t,
      Math.round((4.95 * F) / FRAME) * FRAME,
    ]);
  });

  it("never moves a marker before time 0", async () => {
    const { drag, sent } = setup();
    drag.handle(pointer("down", 200));
    drag.handle(pointer("move", -400));
    drag.handle(pointer("up", -400));
    await drag.settled();
    expect(sent("timeline.move_marker").map((r) => (r.params as { t: number }).t)).toEqual([0]);
  });

  it("rolls the drag back on Esc, a lost pointer or a lost window", async () => {
    const { drag, transport, guides } = setup();
    drag.handle(pointer("down", 200));
    drag.handle(pointer("move", 300));
    await flush();
    expect(drag.cancel()).toBe(true);
    await drag.settled();
    expect(transport.requests.map((r) => r.method)).toEqual([
      "edit.begin_tx",
      "timeline.move_marker",
      "edit.cancel_tx",
    ]);
    expect(guides).toHaveBeenLastCalledWith([]);
    expect(drag.cancel()).toBe(false);
    // The pointer's later events are the timeline's again.
    expect(drag.handle(pointer("up", 300))).toBe(false);
  });

  it("tells the person when the engine refuses the move, and rolls back", async () => {
    const { drag, refused, transport } = setup({
      "timeline.move_marker": () => {
        throw new RpcError("invalid", "a marker cannot be before time 0");
      },
    });
    drag.handle(pointer("down", 200));
    drag.handle(pointer("move", 300));
    await flush();
    drag.handle(pointer("up", 300));
    await drag.settled();
    expect(refused).toHaveBeenCalledTimes(1);
    expect(transport.requests.at(-1)?.method).toBe("edit.cancel_tx");
  });
});
