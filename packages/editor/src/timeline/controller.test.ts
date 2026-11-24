// SPDX-License-Identifier: GPL-3.0-or-later
// TimelineController with a scripted engine: how intents land in the stores and the engine.
// The full flows run against a real engine in `timeline.integration.test.ts`.
import {
  MockTransport,
  type Project,
  type RecordedRequest,
  RpcClient,
  RpcError,
} from "@cutweave/protocol";
import type { HitResult, TimelinePointerEvent } from "@cutweave/timeline";
import { beforeEach, describe, expect, it } from "vitest";
import { createEngineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { createDocStore } from "../state/docStore.ts";
import { createToastStore } from "../state/toastStore.ts";
import { createViewStore } from "../state/viewStore.ts";
import { TimelineController } from "./controller.ts";
import { describeRefusal, translator } from "./errors.ts";

const F = 705_600_000;

const clip = (id: string, start: number, group: string | null = null) => ({
  kind: "clip",
  id,
  name: null,
  start,
  duration: 2 * F,
  enabled: true,
  linked_group: group,
  media: "med_a",
  source_in: 0,
  audio: { gain: 0 },
  visual: {},
  custom: {},
});

const track = (id: string, kind: string, items: object[]) => ({
  id,
  kind,
  name: id,
  items: Object.fromEntries(items.map((i) => [(i as { id: string }).id, i])),
  muted: false,
  hidden: false,
  locked: false,
  solo: false,
  magnetic: false,
  gain: 0,
  custom: {},
});

const PROJECT = {
  meta: { name: "P" },
  settings: {},
  media: {
    med_a: { id: "med_a", kind: "video", path_abs: "/m/a.mp4", probe: { duration: 10 * F } },
  },
  sequences: {
    seq_1: {
      id: "seq_1",
      name: "Main",
      format: { width: 1920, height: 1080, frame_rate: { num: 30, den: 1 } },
      track_order: ["trk_v", "trk_a"],
      tracks: {
        trk_v: track("trk_v", "video", [clip("itm_a", F, "grp_1"), clip("itm_b", 5 * F)]),
        trk_a: track("trk_a", "audio", [clip("itm_a2", F, "grp_1")]),
      },
      custom: {},
    },
  },
  custom: {},
} as unknown as Project;

function setup(handlers: Record<string, (params: unknown) => unknown> = {}) {
  const transport = new MockTransport()
    .handle("clip.split", () => ({ left: "l", right: "r", at: 0, linked: [], right_group: null }))
    .handle("edit.begin_tx", () => ({ tx: "tx_9", kind: "batch", label: "Split clip", rev: 1 }))
    .handle("edit.commit_tx", () => ({
      tx: "tx_9",
      kind: "batch",
      changed: true,
      rev: 2,
      committed_rev: 2,
    }))
    .handle("edit.cancel_tx", () => ({ tx: "tx_9", kind: "batch", rev: 2 }))
    .handle("item.delete", (p) => ({ deleted: [...(p as { items: string[] }).items, "itm_a2"] }));
  for (const [name, fn] of Object.entries(handlers)) transport.handle(name, fn);
  const doc = createDocStore();
  doc.getState().resync({ rev: 1, committed_rev: 1, project: PROJECT });
  const view = createViewStore();
  view.setState({ zoom: 100 });
  const engine = createEngineStore();
  engine.setState({ rpc: new RpcClient(transport), connection: "ready" });
  const toasts = createToastStore();
  const inRect: string[] = [];
  const under = { hit: null as HitResult | null };
  const controller = new TimelineController({
    doc,
    view,
    engine,
    toasts,
    t: translator(i18n.t.bind(i18n)),
  });
  controller.attach({
    getViewState: () => ({ pxPerSec: 100, scrollX: 0, scrollY: 0 }),
    itemsInRect: () => inRect,
    hitTest: () => under.hit ?? hit("lane", 0),
  });
  return { transport, doc, view, engine, toasts, controller, inRect, under };
}

const hit = (
  zone: HitResult["zone"],
  time: number,
  trackId: string | null = "trk_v",
): HitResult => ({
  zone,
  time,
  trackId,
  trackIndex: trackId === null ? -1 : 0,
  itemId: null,
  itemIndex: -1,
  keyframeIndex: -1,
  keyframeTime: null,
  toggle: null,
});

function pointer(
  controller: TimelineController,
  type: TimelinePointerEvent["type"],
  x: number,
  h: HitResult,
  extra: Partial<TimelinePointerEvent> = {},
): void {
  controller.handlePointer({
    type,
    x,
    y: 40,
    hit: h,
    button: 0,
    buttons: 1,
    shiftKey: false,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    original: {} as MouseEvent,
    ...extra,
  });
}

const calls = (requests: readonly RecordedRequest[]) => requests.map((r) => r.method);
const items = (ids: string[]) => ids.map((id) => ({ kind: "item", id }));

describe("TimelineController media drops (WP-1M.03)", () => {
  const card = { id: "med_a", kind: "video", duration: 10 * F } as const;
  const insert = () => ({ at: 0, duration: 0, clips: ["itm_new", "itm_new_audio"] });

  it("sends timeline.insert with overlap new_track at the snapped time, then selects the clip", async () => {
    const t = setup({ "timeline.insert": insert });
    t.under.hit = hit("lane", 3 * F + 1000, "trk_v");
    expect(t.controller.mediaDragOver(400, 40, card)).toBe(true);
    expect(t.controller.state.mode.name).toBe("dropping-media");
    expect(t.controller.overlay.getState().snapGuides).toEqual([3 * F]);
    t.controller.mediaDrop(400, 40, card);
    await t.controller.settled();
    const sent = t.transport.requests.filter((r) => r.method === "timeline.insert");
    expect(sent).toHaveLength(1);
    expect(sent[0]?.params).toEqual({
      sequence: "seq_1",
      track: "trk_v",
      media: "med_a",
      at: 3 * F,
      overlap: "new_track",
    });
    expect(t.controller.overlay.getState().snapGuides).toEqual([]);
    expect(t.view.getState().selection).toEqual(items(["itm_new"]));
  });

  it("puts a sound on the audio track even over a video lane", async () => {
    const t = setup({ "timeline.insert": insert });
    t.under.hit = hit("lane", 8 * F, "trk_v");
    t.controller.mediaDrop(400, 40, { id: "med_s", kind: "audio", duration: 2 * F });
    await t.controller.settled();
    const sent = t.transport.requests.find((r) => r.method === "timeline.insert");
    expect((sent?.params as { track: string } | undefined)?.track).toBe("trk_a");
  });

  it("shows the engine's refusal as a toast and leaves the selection alone", async () => {
    const t = setup({
      "timeline.insert": () => {
        throw new RpcError("locked", "track trk_v is locked");
      },
    });
    t.under.hit = hit("lane", 8 * F, "trk_v");
    t.controller.mediaDrop(400, 40, card);
    await t.controller.settled();
    const [toast] = t.toasts.getState().notices;
    expect(toast?.title).toBe("Clip not added");
    expect(toast?.message).toBe("The track is locked. Unlock it to edit its clips.");
    expect(t.view.getState().selection).toEqual([]);
  });

  it("does nothing without a live view, and leaving clears the guide", () => {
    const t = setup();
    t.controller.attach(null);
    expect(t.controller.mediaDragOver(400, 40, card)).toBe(false);
    t.controller.attach({
      getViewState: () => ({ pxPerSec: 100, scrollX: 0, scrollY: 0 }),
      itemsInRect: () => [],
      hitTest: () => hit("lane", 2 * F, "trk_v"),
    });
    t.controller.mediaDragOver(400, 40, card);
    expect(t.controller.overlay.getState().snapGuides).toEqual([2 * F]);
    t.controller.mediaDragLeave();
    expect(t.controller.overlay.getState().snapGuides).toEqual([]);
    expect(t.controller.state.mode.name).toBe("idle");
  });
});

describe("TimelineController", () => {
  let t: ReturnType<typeof setup>;
  beforeEach(() => {
    t = setup();
  });

  it("selects through the view store, replacing other selected entities", () => {
    t.view.setState({ selection: [{ kind: "media", id: "med_a" as never }] });
    pointer(t.controller, "down", 300, { ...hit("body", 2 * F), itemId: "itm_a", itemIndex: 0 });
    expect(t.view.getState().selection).toEqual(items(["itm_a"]));
  });

  it("adds the marquee's items to the selection with shift", () => {
    t.view.setState({ selection: items(["itm_b"]) as never });
    t.inRect.push("itm_a", "itm_b");
    pointer(t.controller, "down", 450, hit("lane", 3.5 * F), { shiftKey: true });
    pointer(t.controller, "move", 480, hit("lane", 3.8 * F), { shiftKey: true });
    expect(t.controller.overlay.getState().marquee).not.toBeNull();
    pointer(t.controller, "up", 480, hit("lane", 3.8 * F), { shiftKey: true });
    expect(t.view.getState().selection).toEqual(items(["itm_b", "itm_a"]));
    expect(t.controller.overlay.getState().marquee).toBeNull();
  });

  it("scrubs the ruler into the view store's playhead", () => {
    pointer(t.controller, "down", 300, hit("ruler", 2 * F + 5, null));
    expect(t.view.getState().playhead).toBe(2 * F);
    pointer(t.controller, "move", 350, hit("ruler", 2.5 * F, null));
    expect(t.view.getState().playhead).toBe(2.5 * F);
  });

  it("with a transport, ruler scrubbing seeks through it (view.seek, WP-1M.04)", () => {
    const seeks: number[] = [];
    const controller = new TimelineController({
      doc: t.doc,
      view: t.view,
      engine: t.engine,
      toasts: t.toasts,
      t: translator(i18n.t.bind(i18n)),
      seek: (at) => seeks.push(at),
    });
    controller.attach({
      getViewState: () => ({ pxPerSec: 100, scrollX: 0, scrollY: 0 }),
      itemsInRect: () => [],
      hitTest: () => hit("lane", 0),
    });
    pointer(controller, "down", 300, hit("ruler", 2 * F + 5, null));
    pointer(controller, "move", 350, hit("ruler", 2.5 * F, null));
    expect(seeks).toEqual([2 * F, 2.5 * F]);
  });

  it("pans with the middle button through the view store's scroll", () => {
    pointer(t.controller, "down", 500, hit("lane", 4 * F), { button: 1 });
    pointer(t.controller, "move", 400, hit("lane", 3 * F), { button: 1 });
    expect(t.view.getState().scroll.x).toBe(F);
  });

  it("toggles snapping in the view store with N", () => {
    expect(t.controller.handleKey({ key: "n" })).toBe(true);
    expect(t.view.getState().snapping).toBe(false);
    t.controller.toggleSnapping();
    expect(t.view.getState().snapping).toBe(true);
  });

  it("reports unhandled keys so the panel leaves them alone", () => {
    expect(t.controller.handleKey({ key: "x" })).toBe(false);
    expect(t.controller.handleKey({ key: "s", ctrlKey: true })).toBe(false);
    expect(t.controller.handleKey({ key: "Delete" })).toBe(false); // nothing selected
  });

  it("splits one clip with a single command", async () => {
    t.view.setState({ playhead: 2 * F, selection: items(["itm_a"]) as never });
    t.controller.split();
    await t.controller.settled();
    expect(t.transport.requests).toEqual([
      { method: "clip.split", params: { item: "itm_a", at: 2 * F } },
    ]);
  });

  it("splits several unlinked clips in one batch transaction: one undo step", async () => {
    // A selected audio clip that is not linked to itm_a and also spans the playhead.
    const sequence = PROJECT.sequences["seq_1" as never] as unknown as Record<string, unknown>;
    t.doc.getState().resync({
      rev: 2,
      committed_rev: 2,
      project: {
        ...PROJECT,
        sequences: {
          seq_1: {
            ...sequence,
            tracks: {
              trk_v: track("trk_v", "video", [clip("itm_a", F)]),
              trk_a: track("trk_a", "audio", [clip("itm_c", F)]),
            },
          },
        },
      } as unknown as Project,
    });
    t.view.setState({ playhead: 2 * F, selection: items(["itm_a", "itm_c"]) as never });
    t.controller.split();
    await t.controller.settled();
    expect(calls(t.transport.requests)).toEqual([
      "edit.begin_tx",
      "clip.split",
      "clip.split",
      "edit.commit_tx",
    ]);
    expect(t.transport.requests[0]?.params).toEqual({ kind: "batch", label: "Split clip" });
    expect(t.transport.requests[1]?.meta).toEqual({ tx: "tx_9" });
  });

  it("rolls a failed batch split back and shows a toast", async () => {
    const failing = setup({
      "clip.split": () => {
        throw new RpcError("locked", "track is locked");
      },
    });
    const sequence = PROJECT.sequences["seq_1" as never] as unknown as Record<string, unknown>;
    failing.doc.getState().resync({
      rev: 2,
      committed_rev: 2,
      project: {
        ...PROJECT,
        sequences: {
          seq_1: {
            ...sequence,
            tracks: {
              trk_v: track("trk_v", "video", [clip("itm_a", F)]),
              trk_a: track("trk_a", "audio", [clip("itm_c", F)]),
            },
          },
        },
      } as unknown as Project,
    });
    failing.view.setState({ playhead: 2 * F, selection: items(["itm_a", "itm_c"]) as never });
    failing.controller.split();
    await failing.controller.settled();
    expect(calls(failing.transport.requests)).toEqual([
      "edit.begin_tx",
      "clip.split",
      "edit.cancel_tx",
    ]);
    expect(failing.toasts.getState().notices[0]?.title).toBe("Clip not split");
  });

  it("deletes the selection and drops the deleted items (and partners) from it", async () => {
    t.view.setState({
      selection: [...items(["itm_a", "itm_a2"]), { kind: "media", id: "med_a" }] as never,
    });
    expect(t.controller.handleKey({ key: "Backspace" })).toBe(true);
    await t.controller.settled();
    expect(t.transport.requests[0]).toEqual({
      method: "item.delete",
      params: { items: ["itm_a", "itm_a2"] },
    });
    expect(t.view.getState().selection).toEqual([{ kind: "media", id: "med_a" }]);
  });

  it("shows a toast when the engine refuses a delete", async () => {
    const locked = setup({
      "item.delete": () => {
        throw new RpcError("locked", "track `trk_v` is locked");
      },
    });
    locked.view.setState({ selection: items(["itm_b"]) as never });
    locked.controller.deleteSelection();
    await locked.controller.settled();
    expect(locked.toasts.getState().notices).toEqual([
      {
        id: expect.any(Number),
        tone: "warning",
        title: "Clip not deleted",
        message: "The track is locked. Unlock it to edit its clips.",
      },
    ]);
  });

  it("does not open a gesture without an engine", () => {
    t.engine.setState({ rpc: null, connection: "none" });
    pointer(t.controller, "down", 300, { ...hit("body", 2 * F), itemId: "itm_a", itemIndex: 0 });
    pointer(t.controller, "move", 340, { ...hit("body", 2.4 * F), itemId: "itm_a", itemIndex: 0 });
    expect(t.controller.state.mode.name).toBe("idle");
    expect(t.transport.requests).toEqual([]);
  });
});

describe("refusal messages", () => {
  const tr = translator(i18n.t.bind(i18n));
  it("says what happened and what to do next", () => {
    expect(describeRefusal(new RpcError("invalid", "item `a` would overlap item `b`"), tr)).toBe(
      "It would overlap another clip on that track. Drop it on a free spot.",
    );
    expect(
      describeRefusal(
        new RpcError("conflict", "x", undefined, {
          reason: "item_in_gesture",
          retry_after_ms: 300,
        }),
        tr,
      ),
    ).toBe("Someone else is editing this clip right now. Try again in a moment.");
    expect(describeRefusal(new RpcError("conflict", "stale"), tr)).toBe(
      "The project changed while you were editing. Try again.",
    );
    expect(describeRefusal(new RpcError("not_found", "no item"), tr)).toBe(
      "The clip no longer exists.",
    );
    expect(
      describeRefusal(
        new RpcError("not_found", "transaction `tx_1` not found", undefined, {
          kind: "transaction",
        }),
        tr,
      ),
    ).toBe("The edit was interrupted and undone. Try again.");
    expect(describeRefusal(new RpcError("invalid", "would read past the end"), tr)).toBe(
      "The engine refused the edit: would read past the end",
    );
  });
});
