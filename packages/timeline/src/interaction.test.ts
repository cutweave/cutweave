// SPDX-License-Identifier: GPL-3.0-or-later
// The interaction state machine (WP-1M.06): every transition, with the intents it emits.
import { describe, expect, it } from "vitest";
import { generateTimeline } from "./fixtures.ts";
import type { HitResult, HitZone } from "./hit-test.ts";
import {
  DRAG_THRESHOLD_PX,
  fromPointerEvent,
  type Intent,
  type InteractionEnv,
  type InteractionEvent,
  type InteractionState,
  initialState,
  isBusy,
  isGestureOpen,
  type PointerInput,
  reduce,
  trimLimits,
} from "./interaction.ts";
import type { TimelineItem, TimelineModel } from "./model.ts";
import { FLICKS_PER_SECOND as F } from "./time.ts";
import type { TimelinePointerEvent } from "./timeline-view.ts";

// 30 fps; 100 px per second, so one pixel is 7,056,000 flicks and a frame about 3.3 px.
const FRAME = F / 30;
const PX = F / 100;
const HEADER = 100;
const xAt = (t: number): number => HEADER + t / PX;
const timeAt = (x: number): number => Math.round((x - HEADER) * PX);
const sec = (s: number): number => Math.round(s * F);

const A: TimelineItem = {
  id: "a",
  trackId: "v1",
  kind: "media",
  start: sec(1),
  duration: sec(2),
  label: "A",
  linkedGroup: "g1",
  source: { sourceIn: sec(0.5), mediaDuration: sec(10) },
};
const A_AUDIO: TimelineItem = {
  id: "a-audio",
  trackId: "a1",
  kind: "audio",
  start: sec(1),
  duration: sec(2),
  label: "A",
  linkedGroup: "g1",
  source: { sourceIn: sec(0.5), mediaDuration: sec(10) },
};
const B: TimelineItem = {
  id: "b",
  trackId: "v1",
  kind: "media",
  start: sec(5),
  duration: sec(2),
  label: "B",
  source: { sourceIn: 0, mediaDuration: sec(4) },
};
const C: TimelineItem = {
  id: "c",
  trackId: "v2",
  kind: "media",
  start: 0,
  duration: sec(1),
  label: "C",
  source: { sourceIn: 0, mediaDuration: null, still: true },
};

const MODEL: TimelineModel = {
  fps: { num: 30, den: 1 },
  tracks: [
    { id: "v2", name: "V2", kind: "media" },
    { id: "v1", name: "V1", kind: "media" },
    { id: "a1", name: "A1", kind: "audio" },
  ],
  items: [C, A, B, A_AUDIO],
};

const TRACK_Y: Record<string, number> = { v2: 50, v1: 100, a1: 150 };

function start(env: Partial<InteractionEnv> = {}): InteractionState {
  return initialState({ model: MODEL, pxPerSec: 100, playhead: sec(10), ...env });
}

interface At {
  zone?: HitZone;
  item?: TimelineItem | null;
  track?: string | null;
  y?: number;
  button?: number;
  shift?: boolean;
  alt?: boolean;
  ctrl?: boolean;
  meta?: boolean;
}

/** A pointer at canvas x with the hit result `TimelineView` would report. */
function pointer(x: number, o: At = {}): PointerInput {
  const item = o.item ?? null;
  const trackId = o.track !== undefined ? o.track : (item?.trackId ?? null);
  const zone: HitZone = o.zone ?? (item ? "body" : trackId ? "lane" : "none");
  const hit: HitResult = {
    zone,
    time: timeAt(x),
    trackId,
    trackIndex: trackId === null ? -1 : MODEL.tracks.findIndex((t) => t.id === trackId),
    itemId: item?.id ?? null,
    itemIndex: item ? MODEL.items.indexOf(item) : -1,
    keyframeIndex: -1,
    keyframeTime: null,
    toggle: null,
  };
  return {
    x,
    y: o.y ?? (trackId !== null ? (TRACK_Y[trackId] ?? 0) : 10),
    hit,
    button: o.button ?? 0,
    shiftKey: o.shift ?? false,
    altKey: o.alt ?? false,
    ctrlKey: o.ctrl ?? false,
    metaKey: o.meta ?? false,
  };
}

const down = (x: number, o?: At): InteractionEvent => ({ type: "pointer-down", ...pointer(x, o) });
const move = (x: number, o?: At): InteractionEvent => ({ type: "pointer-move", ...pointer(x, o) });
const up = (x: number, o?: At): InteractionEvent => ({ type: "pointer-up", ...pointer(x, o) });
const key = (k: string, mods: Partial<Record<"ctrlKey" | "metaKey" | "altKey", boolean>> = {}) =>
  ({ type: "key", key: k, ...mods }) as InteractionEvent;

/** Runs events in order; returns the final state and every intent emitted, in order. */
function run(
  state: InteractionState,
  ...events: InteractionEvent[]
): { state: InteractionState; intents: Intent[] } {
  const intents: Intent[] = [];
  let s = state;
  for (const e of events) {
    const t = reduce(s, e);
    s = t.state;
    intents.push(...t.intents);
  }
  return { state: s, intents };
}

const kinds = (intents: readonly Intent[]) => intents.map((i) => i.kind);
const updates = (intents: readonly Intent[]) =>
  intents.filter((i) => i.kind === "move" || i.kind === "trim") as Extract<
    Intent,
    { kind: "move" | "trim" }
  >[];
const aligned = (t: number) => t % FRAME === 0;

/** Press the body of `item` at its middle (on its track). */
const bodyX = (item: TimelineItem) => xAt(item.start + item.duration / 2);

// ---- idle and hovering --------------------------------------------------------------------

describe("idle and hovering", () => {
  it("starts idle with the given environment", () => {
    const s = start({ snapping: false });
    expect(s.mode.name).toBe("idle");
    expect(s.env.snapping).toBe(false);
    expect(isBusy(s)).toBe(false);
  });

  it("enters hovering on a pointer move and records the zone and item", () => {
    const { state, intents } = run(start(), move(bodyX(A), { item: A }));
    expect(state.mode).toEqual({ name: "hovering", zone: "body", itemId: "a" });
    expect(intents).toEqual([]);
  });

  it("keeps the same state object while the hovered zone and item do not change", () => {
    const s = run(start(), move(bodyX(A), { item: A })).state;
    expect(reduce(s, move(bodyX(A) + 1, { item: A })).state).toBe(s);
  });

  it("goes back to idle when the pointer leaves", () => {
    const { state } = run(start(), move(bodyX(A), { item: A }), { type: "pointer-leave" });
    expect(state.mode.name).toBe("idle");
  });

  it("merges an env update without intents", () => {
    const t = reduce(start(), { type: "env", env: { playhead: 42 } });
    expect(t.state.env.playhead).toBe(42);
    expect(t.state.env.model).toBe(MODEL);
    expect(t.intents).toEqual([]);
  });

  it("ignores the secondary button and presses on track headers", () => {
    const right = reduce(start(), down(bodyX(A), { item: A, button: 2 }));
    expect(right.intents).toEqual([]);
    expect(right.state.mode.name).toBe("idle");
    const header = reduce(start(), down(20, { zone: "header", track: "v1" }));
    expect(header.state.mode.name).toBe("hovering");
    expect(header.intents).toEqual([]);
  });

  it("maps TimelineView pointer events to machine events", () => {
    const base = { x: 1, y: 2, hit: pointer(1).hit, button: 0, buttons: 1 };
    const mods = { shiftKey: false, ctrlKey: false, altKey: false, metaKey: false };
    const ev = (type: TimelinePointerEvent["type"]) =>
      fromPointerEvent({ type, ...base, ...mods, original: {} as MouseEvent });
    expect(ev("down")?.type).toBe("pointer-down");
    expect(ev("move")?.type).toBe("pointer-move");
    expect(ev("up")?.type).toBe("pointer-up");
    expect(ev("leave")).toEqual({ type: "pointer-leave" });
    expect(ev("cancel")).toEqual({ type: "pointer-cancel" });
    expect(ev("dblclick")).toBeNull();
  });
});

// ---- selection ----------------------------------------------------------------------------

describe("selection", () => {
  it("selects only the pressed clip and opens no gesture for a plain click", () => {
    const s = start({ selection: ["b"] });
    const pressed = reduce(s, down(bodyX(A), { item: A }));
    expect(pressed.intents).toEqual([{ kind: "select", ids: ["a"] }]);
    expect(pressed.state.mode.name).toBe("moving");
    const released = reduce(pressed.state, up(bodyX(A), { item: A }));
    expect(released.intents).toEqual([]);
    expect(released.state.mode.name).toBe("hovering");
    expect(released.state.env.selection).toEqual(["a"]);
  });

  it("adds a clip with shift-click without starting a drag", () => {
    const { state, intents } = run(
      start({ selection: ["b"] }),
      down(bodyX(A), { item: A, shift: true }),
      move(bodyX(A) + 50, { item: A, shift: true }),
    );
    expect(intents).toEqual([{ kind: "select", ids: ["b", "a"] }]);
    expect(state.mode.name).toBe("hovering");
  });

  it("removes a selected clip with shift-click", () => {
    const { intents } = run(
      start({ selection: ["a", "b"] }),
      down(bodyX(A), { item: A, shift: true }),
    );
    expect(intents).toEqual([{ kind: "select", ids: ["b"] }]);
  });

  it("toggles with ctrl- and cmd-click like shift-click", () => {
    expect(run(start(), down(bodyX(A), { item: A, ctrl: true })).intents).toEqual([
      { kind: "select", ids: ["a"] },
    ]);
    expect(
      run(start({ selection: ["a"] }), down(bodyX(A), { item: A, meta: true })).intents,
    ).toEqual([{ kind: "select", ids: [] }]);
  });

  it("keeps a multi-selection on press and collapses it to the clip on a click release", () => {
    const s = start({ selection: ["a", "b"] });
    const pressed = reduce(s, down(bodyX(A), { item: A }));
    expect(pressed.intents).toEqual([]);
    const released = reduce(pressed.state, up(bodyX(A), { item: A }));
    expect(released.intents).toEqual([{ kind: "select", ids: ["a"] }]);
  });

  it("deselects everything on a click in an empty lane", () => {
    const { state, intents } = run(
      start({ selection: ["a", "b"] }),
      down(xAt(sec(4)), { track: "v1" }),
      up(xAt(sec(4)), { track: "v1" }),
    );
    expect(intents).toEqual([{ kind: "select", ids: [] }]);
    expect(state.env.selection).toEqual([]);
  });

  it("keeps the selection on a shift-click in an empty lane, and emits nothing when empty", () => {
    expect(
      run(
        start({ selection: ["a"] }),
        down(xAt(sec(4)), { track: "v1", shift: true }),
        up(xAt(sec(4)), { track: "v1", shift: true }),
      ).intents,
    ).toEqual([]);
    expect(
      run(start(), down(xAt(sec(4)), { track: "v1" }), up(xAt(sec(4)), { track: "v1" })).intents,
    ).toEqual([]);
  });

  it("draws a marquee once the pointer travels and selects inside it on release", () => {
    const x0 = xAt(sec(4));
    const { state, intents } = run(
      start(),
      down(x0, { track: "v1", y: 90 }),
      move(x0 + 1, { track: "v1", y: 90 }),
      move(x0 + 40, { track: "a1", y: 160 }),
      up(x0 + 40, { track: "a1", y: 160 }),
    );
    const rect = { x: x0, y: 90, w: 40, h: 70 };
    expect(intents).toEqual([
      { kind: "marquee", rect },
      { kind: "select-rect", rect, additive: false },
      { kind: "marquee", rect: null },
    ]);
    expect(state.mode.name).toBe("hovering");
  });

  it("adds the marquee's items with shift and normalizes a leftward drag", () => {
    const x0 = xAt(sec(4));
    const { intents } = run(
      start(),
      down(x0, { track: "v1", y: 90, shift: true }),
      move(x0 - 30, { track: "v1", y: 80, shift: true }),
      up(x0 - 30, { track: "v1", y: 80, shift: true }),
    );
    expect(intents[1]).toEqual({
      kind: "select-rect",
      rect: { x: x0 - 30, y: 80, w: 30, h: 10 },
      additive: true,
    });
  });

  it("clears the marquee and returns to idle on Esc", () => {
    const x0 = xAt(sec(4));
    const { state, intents } = run(
      start(),
      down(x0, { track: "v1" }),
      move(x0 + 20, { track: "v1" }),
      key("Escape"),
    );
    expect(intents.at(-1)).toEqual({ kind: "marquee", rect: null });
    expect(state.mode.name).toBe("idle");
  });
});

// ---- moving -------------------------------------------------------------------------------

describe("moving", () => {
  const x = bodyX(A);

  it("stays a click below the drag threshold", () => {
    const { state, intents } = run(
      start(),
      down(x, { item: A }),
      move(x + DRAG_THRESHOLD_PX - 1, { item: A }),
    );
    expect(kinds(intents)).toEqual(["select"]);
    expect(isGestureOpen(state)).toBe(false);
  });

  it("opens one gesture when the pointer crosses the threshold, then sends updates", () => {
    const { state, intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(x + 10, { item: A }),
      move(x + 20, { item: A }),
    );
    expect(kinds(intents)).toEqual(["select", "begin-gesture", "move", "move"]);
    expect(intents[1]).toEqual({ kind: "begin-gesture", gesture: "move", item: "a" });
    expect(isGestureOpen(state)).toBe(true);
    expect(isBusy(state)).toBe(true);
  });

  it("drags a clip 100 px to a frame-aligned start one second later", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(x + 100, { item: A }),
    );
    expect(updates(intents).at(-1)).toEqual({
      kind: "move",
      item: "a",
      toTrack: "v1",
      at: A.start + sec(1),
      unlink: false,
    });
  });

  it("sends only frame-aligned starts for any pixel offset", () => {
    let s = run(start({ snapping: false }), down(x, { item: A })).state;
    for (let dx = 4; dx < 80; dx += 1.37) {
      const t = reduce(s, move(x + dx, { item: A }));
      s = t.state;
      for (const u of updates(t.intents)) expect(aligned(u.at)).toBe(true);
    }
  });

  it("does not resend while the snapped position stays on the same frame", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(x + 10, { item: A }),
      move(x + 10.2, { item: A }),
      move(x + 10.4, { item: A }),
    );
    expect(updates(intents)).toHaveLength(1);
  });

  it("snaps the clip's end to the playhead within 8 px and shows a guide", () => {
    // A ends at 3 s; the playhead at 4 s. Dragging 96 px puts the end 4 px before it.
    const { intents } = run(
      start({ playhead: sec(4) }),
      down(x, { item: A }),
      move(x + 96, { item: A }),
    );
    expect(updates(intents).at(-1)?.at).toBe(sec(4) - A.duration);
    expect(intents).toContainEqual({ kind: "snap-guides", times: [sec(4)] });
  });

  it("snaps the clip's start to a neighbour's end", () => {
    // C ends at 1 s on V2; drag A so its start comes 5 px after it.
    const { intents } = run(start(), down(x, { item: A }), move(x + 5, { item: A }));
    expect(updates(intents)).toEqual([]); // 1 s is A's own start: it snaps back home
    const far = run(start(), down(x, { item: A }), move(x + 195, { item: A })).intents;
    // A's end (3 s + 1.95 s = 4.95 s) is 5 px from B's start (5 s).
    expect(updates(far).at(-1)?.at).toBe(sec(5) - A.duration);
  });

  it("never snaps to its own linked partner", () => {
    // Without C, the only target near 1 s would be the partner's start: it must be ignored.
    const model = { ...MODEL, items: [A, B, A_AUDIO] };
    const { intents } = run(start({ model }), down(x, { item: A }), move(x + 4, { item: A }));
    const u = updates(intents).at(-1);
    expect(u?.at).not.toBe(A.start);
    expect(u?.at).toBe(A.start + FRAME);
  });

  it("does not snap with snapping off, but still aligns to frames", () => {
    const { intents } = run(
      start({ playhead: sec(4), snapping: false }),
      down(x, { item: A }),
      move(x + 96, { item: A }),
    );
    const u = updates(intents).at(-1);
    expect(u?.at).not.toBe(sec(4) - A.duration);
    expect(aligned(u?.at ?? 1)).toBe(true);
    expect(kinds(intents)).not.toContain("snap-guides");
  });

  it("clamps at the timeline start when dragged out past the left edge", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(-500, { zone: "none", track: null, y: -40 }),
    );
    expect(updates(intents).at(-1)).toMatchObject({ at: 0, toTrack: "v1" });
  });

  it("keeps its track when the pointer leaves the canvas or crosses the ruler", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(x + 300, { zone: "ruler", track: null, y: 5 }),
      move(x + 5000, { zone: "none", track: null, y: 2000 }),
    );
    for (const u of updates(intents)) expect(u).toMatchObject({ toTrack: "v1" });
    expect(updates(intents).at(-1)?.at).toBe(A.start + sec(50));
  });

  it("moves to another track of the same kind", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(x + 30, { track: "v2" }),
    );
    expect(updates(intents).at(-1)).toMatchObject({ toTrack: "v2" });
  });

  it("does not move a video clip onto an audio track", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(x + 30, { track: "v2" }),
      move(x + 40, { track: "a1" }),
    );
    expect(updates(intents).at(-1)).toMatchObject({ toTrack: "v2" });
  });

  it("unlinks while Alt is held and stays unlinked for the rest of the drag", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(x + 10, { item: A }),
      move(x + 20, { item: A, alt: true }),
      move(x + 30, { item: A }),
    );
    expect(updates(intents).map((u) => u.unlink)).toEqual([false, true, true]);
  });

  it("commits on release, returns to hovering and clears the guide", () => {
    const { state, intents } = run(
      start({ playhead: sec(4) }),
      down(x, { item: A }),
      move(x + 96, { item: A }),
      up(x + 96, { item: A }),
    );
    expect(kinds(intents).slice(-2)).toEqual(["commit-gesture", "snap-guides"]);
    expect(intents.at(-1)).toEqual({ kind: "snap-guides", times: [] });
    expect(state.mode.name).toBe("hovering");
    expect(isGestureOpen(state)).toBe(false);
  });

  it("sends the release position before committing when it differs from the last update", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(x + 10, { item: A }),
      up(x + 50, { item: A }),
    );
    expect(kinds(intents).slice(-2)).toEqual(["move", "commit-gesture"]);
    expect(updates(intents).at(-1)?.at).toBe(A.start + sec(0.5));
  });

  it("cancels the gesture on Esc and ignores the rest of the drag", () => {
    const { state, intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(x + 50, { item: A }),
      key("Escape"),
      move(x + 80, { item: A }),
      up(x + 80, { item: A }),
    );
    expect(kinds(intents)).toEqual(["select", "begin-gesture", "move", "cancel-gesture"]);
    expect(state.mode.name).toBe("hovering");
  });

  it("returns to idle on Esc before the threshold without gesture intents", () => {
    const { state, intents } = run(start(), down(x, { item: A }), key("Escape"));
    expect(kinds(intents)).toEqual(["select"]);
    expect(state.mode.name).toBe("idle");
  });

  it("cancels when the browser takes the pointer away", () => {
    const { intents } = run(
      start({ playhead: sec(4) }),
      down(x, { item: A }),
      move(x + 96, { item: A }),
      { type: "pointer-cancel" },
    );
    expect(kinds(intents).slice(-2)).toEqual(["cancel-gesture", "snap-guides"]);
  });

  it("leaves the gesture without intents of its own when the host aborted it", () => {
    const { state, intents } = run(
      start({ snapping: false }),
      down(x, { item: A }),
      move(x + 50, { item: A }),
      { type: "gesture-aborted" },
      up(x + 50, { item: A }),
    );
    expect(kinds(intents)).toEqual(["select", "begin-gesture", "move"]);
    expect(state.mode.name).toBe("idle");
  });

  it("ignores a second press, split, delete and pointer-leave during a drag", () => {
    const s = run(
      start({ selection: ["a"] }),
      down(x, { item: A }),
      move(x + 50, { item: A }),
    ).state;
    for (const e of [
      down(x + 60, { item: B }),
      key("s"),
      key("Delete"),
      { type: "split" },
      { type: "delete" },
      { type: "pointer-leave" },
    ] as InteractionEvent[]) {
      const t = reduce(s, e);
      expect(t.state).toBe(s);
      expect(t.intents).toEqual([]);
    }
  });

  it("toggles snapping with N during a drag; the next move uses the new setting", () => {
    const s = run(
      start({ playhead: sec(4) }),
      down(x, { item: A }),
      move(x + 50, { item: A }),
    ).state;
    const toggled = reduce(s, key("n"));
    expect(toggled.intents).toEqual([{ kind: "set-snapping", on: false }]);
    const next = reduce(toggled.state, move(x + 96, { item: A }));
    expect(updates(next.intents)[0]?.at).not.toBe(sec(4) - A.duration);
  });
});

// ---- trimming -----------------------------------------------------------------------------

describe("trimming", () => {
  const left = (item: TimelineItem) => xAt(item.start) + 2;
  const right = (item: TimelineItem) => xAt(item.start + item.duration) - 2;

  it("trims the start edge from the left handle", () => {
    const { state, intents } = run(
      start({ snapping: false }),
      down(left(A), { item: A, zone: "left-edge" }),
      move(left(A) + 30, { item: A, zone: "left-edge" }),
    );
    expect(state.mode.name).toBe("trimming-start");
    expect(intents[1]).toEqual({ kind: "begin-gesture", gesture: "trim", item: "a" });
    expect(updates(intents).at(-1)).toEqual({
      kind: "trim",
      item: "a",
      edge: "start",
      at: A.start + sec(0.3),
      unlink: false,
    });
  });

  it("trims the end edge from the right handle", () => {
    const { state, intents } = run(
      start({ snapping: false }),
      down(right(A), { item: A, zone: "right-edge" }),
      move(right(A) - 50, { item: A, zone: "right-edge" }),
      up(right(A) - 50, { item: A, zone: "right-edge" }),
    );
    expect(updates(intents).at(-1)).toMatchObject({ edge: "end", at: sec(2.5) });
    expect(kinds(intents).at(-1)).toBe("commit-gesture");
    expect(state.mode.name).toBe("hovering");
  });

  it("stops the end edge at the end of the media when dragged beyond it", () => {
    // B reads 0..2 s of a 4 s file from 5 s: its end can reach 9 s, no further.
    const { intents } = run(
      start({ snapping: false }),
      down(right(B), { item: B, zone: "right-edge" }),
      move(right(B) + 1000, { zone: "none", track: null }),
    );
    expect(updates(intents).at(-1)?.at).toBe(sec(9));
  });

  it("stops the start edge at the media's first frame", () => {
    // A starts at 1 s reading from 0.5 s: its start can go back to 0.5 s.
    const { intents } = run(
      start({ snapping: false }),
      down(left(A), { item: A, zone: "left-edge" }),
      move(-300, { zone: "none", track: null }),
    );
    expect(updates(intents).at(-1)?.at).toBe(sec(0.5));
  });

  it("stops at the neighbouring clip instead of overlapping it", () => {
    // A's media allows an end at 10.5 s, but B starts at 5 s.
    const { intents } = run(
      start({ snapping: false }),
      down(right(A), { item: A, zone: "right-edge" }),
      move(right(A) + 600, { item: B }),
    );
    expect(updates(intents).at(-1)?.at).toBe(B.start);
  });

  it("keeps at least one frame when the start edge is dragged past the end", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(left(A), { item: A, zone: "left-edge" }),
      move(left(A) + 900, { item: B }),
    );
    expect(updates(intents).at(-1)?.at).toBe(A.start + A.duration - FRAME);
  });

  it("does not bound a still image by its media", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(right(C), { item: C, zone: "right-edge" }),
      move(right(C) + 500, { track: "v2" }),
    );
    expect(updates(intents).at(-1)?.at).toBe(sec(6));
    expect(trimLimits(MODEL, C, "start")).toEqual({ min: 0, max: C.duration - FRAME });
  });

  it("aligns limits that fall between frames inward (ceil for the start, floor for the end)", () => {
    const odd: TimelineItem = { ...B, source: { sourceIn: FRAME / 3, mediaDuration: sec(4) } };
    const model = { ...MODEL, items: [odd] };
    const startLimits = trimLimits(model, odd, "start");
    expect(startLimits.min).toBe(B.start);
    const endLimits = trimLimits(model, odd, "end");
    expect(endLimits.max % FRAME).toBe(0);
    expect(endLimits.max).toBeLessThanOrEqual(B.start - FRAME / 3 + sec(4));
  });

  it("snaps the trimmed edge to the playhead and shows the guide", () => {
    const { intents } = run(
      start({ playhead: sec(2.5) }),
      down(right(A), { item: A, zone: "right-edge" }),
      move(right(A) - 46, { item: A, zone: "right-edge" }),
    );
    expect(updates(intents).at(-1)?.at).toBe(sec(2.5));
    expect(intents).toContainEqual({ kind: "snap-guides", times: [sec(2.5)] });
  });

  it("cancels a trim on Esc", () => {
    const { state, intents } = run(
      start({ snapping: false }),
      down(left(A), { item: A, zone: "left-edge" }),
      move(left(A) + 30, { item: A, zone: "left-edge" }),
      key("Escape"),
    );
    expect(kinds(intents).at(-1)).toBe("cancel-gesture");
    expect(state.mode.name).toBe("idle");
  });

  it("unlinks a trim while Alt is held", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(left(A), { item: A, zone: "left-edge" }),
      move(left(A) + 30, { item: A, zone: "left-edge", alt: true }),
    );
    expect(updates(intents).at(-1)?.unlink).toBe(true);
  });
});

// ---- ruler scrubbing and panning ----------------------------------------------------------

describe("scrubbing the ruler", () => {
  it("seeks to the frame under a press on the ruler", () => {
    const x = xAt(sec(2)) + 1.5; // between frames
    const { state, intents } = run(
      start({ snapping: false }),
      down(x, { zone: "ruler", track: null }),
    );
    expect(state.mode.name).toBe("scrubbing-ruler");
    expect(intents).toHaveLength(1);
    const seek = intents[0] as Extract<Intent, { kind: "seek" }>;
    expect(aligned(seek.at)).toBe(true);
    expect(Math.abs(seek.at - timeAt(x))).toBeLessThanOrEqual(FRAME / 2);
  });

  it("follows the drag and does not repeat a seek to the same frame", () => {
    const x = xAt(sec(2));
    const { intents } = run(
      start({ snapping: false }),
      down(x, { zone: "playhead", track: null }),
      move(x + 0.5, { zone: "ruler", track: null }),
      move(x + 50, { zone: "ruler", track: null }),
      up(x + 50, { zone: "ruler", track: null }),
    );
    expect(intents.map((i) => (i.kind === "seek" ? i.at : null))).toEqual([sec(2), sec(2.5)]);
  });

  it("snaps the playhead to clip edges within 8 px", () => {
    const x = xAt(B.start) - 5;
    const { intents } = run(start(), down(x, { zone: "ruler", track: null }));
    expect(intents).toEqual([{ kind: "seek", at: B.start }]);
  });

  it("clamps at zero when dragged out past the left edge", () => {
    const { intents } = run(
      start({ snapping: false }),
      down(xAt(sec(1)), { zone: "ruler", track: null }),
      move(-400, { zone: "none", track: null }),
    );
    expect(intents.at(-1)).toEqual({ kind: "seek", at: 0 });
  });

  it("restores the playhead on Esc", () => {
    const { state, intents } = run(
      start({ snapping: false, playhead: sec(10) }),
      down(xAt(sec(1)), { zone: "ruler", track: null }),
      key("Escape"),
    );
    expect(intents.at(-1)).toEqual({ kind: "seek", at: sec(10) });
    expect(state.mode.name).toBe("idle");
  });
});

describe("panning", () => {
  it("pans with the middle button from where the drag started", () => {
    const { state, intents } = run(
      start({ scrollX: sec(10), scrollY: 20 }),
      down(500, { item: A, button: 1 }),
      move(450, { item: A, y: 110 }),
    );
    expect(state.mode.name).toBe("panning");
    expect(intents).toEqual([{ kind: "scroll", scrollX: sec(10) + sec(0.5), scrollY: 10 }]);
  });

  it("restores the scroll on Esc and ends on release", () => {
    const s = run(start({ scrollX: sec(10) }), down(500, { button: 1 }), move(400)).state;
    expect(reduce(s, key("Escape")).intents).toEqual([
      { kind: "scroll", scrollX: sec(10), scrollY: 0 },
    ]);
    const released = reduce(s, up(400));
    expect(released.state.mode.name).toBe("hovering");
    expect(released.intents).toEqual([]);
  });
});

// ---- keys: split, delete, snapping --------------------------------------------------------

describe("split, delete and snapping keys", () => {
  it("splits the selected clips that contain the playhead, at its frame", () => {
    const ph = sec(2) + 5; // a few flicks past a frame boundary
    const { intents } = run(start({ selection: ["a", "b"], playhead: ph }), key("s"));
    expect(intents).toEqual([{ kind: "split", items: ["a"], at: sec(2) }]);
  });

  it("splits a selected linked pair once (the engine splits the partner)", () => {
    const { intents } = run(start({ selection: ["a-audio", "a"], playhead: sec(2) }), key("S"));
    expect(intents).toEqual([{ kind: "split", items: ["a-audio"], at: sec(2) }]);
  });

  it("splits the clip under the playhead on the topmost track when nothing is selected", () => {
    expect(run(start({ playhead: sec(0.5) }), key("s")).intents).toEqual([
      { kind: "split", items: ["c"], at: sec(0.5) },
    ]);
    expect(run(start({ playhead: sec(2) }), { type: "split" }).intents).toEqual([
      { kind: "split", items: ["a"], at: sec(2) },
    ]);
  });

  it("does nothing when the playhead is on a clip edge or over no clip", () => {
    expect(run(start({ selection: ["a"], playhead: A.start }), key("s")).intents).toEqual([]);
    expect(run(start({ playhead: sec(4) }), key("s")).intents).toEqual([]);
  });

  it("leaves Ctrl+S, Cmd+S and Alt+S to other handlers", () => {
    for (const mods of [{ ctrlKey: true }, { metaKey: true }, { altKey: true }]) {
      expect(run(start({ playhead: sec(2) }), key("s", mods)).intents).toEqual([]);
    }
  });

  it("deletes the selection with Delete or Backspace, skipping ids that no longer exist", () => {
    const s = start({ selection: ["b", "gone", "a"] });
    expect(run(s, key("Delete")).intents).toEqual([{ kind: "delete", items: ["b", "a"] }]);
    expect(run(s, key("Backspace")).intents).toEqual([{ kind: "delete", items: ["b", "a"] }]);
    expect(run(s, { type: "delete" }).intents).toEqual([{ kind: "delete", items: ["b", "a"] }]);
  });

  it("does nothing on Delete with nothing selected, and on unrelated keys", () => {
    expect(run(start(), key("Delete")).intents).toEqual([]);
    expect(run(start({ selection: ["a"] }), key("x")).intents).toEqual([]);
    expect(run(start(), key("Escape")).intents).toEqual([]);
  });

  it("toggles snapping with N or the toolbar", () => {
    const off = run(start(), key("N"));
    expect(off.intents).toEqual([{ kind: "set-snapping", on: false }]);
    expect(off.state.env.snapping).toBe(false);
    expect(run(off.state, { type: "toggle-snapping" }).intents).toEqual([
      { kind: "set-snapping", on: true },
    ]);
  });
});

// ---- responsiveness -----------------------------------------------------------------------

describe("responsiveness on a 5,000-clip timeline", () => {
  it("starts a drag and handles 60 moves well under 50 ms each", () => {
    const model = generateTimeline();
    const item = model.items[2500] as TimelineItem;
    const s0 = initialState({ model, pxPerSec: 20, playhead: item.start });
    const x0 = HEADER + 400;
    const at = (x: number): PointerInput => ({
      ...pointer(x, { item: null, track: item.trackId }),
      hit: {
        ...pointer(x).hit,
        zone: "body",
        itemId: item.id,
        trackId: item.trackId,
        time: item.start + Math.round((x - x0) * (F / 20)),
      },
    });
    let s = reduce(s0, { type: "pointer-down", ...at(x0) }).state;
    let worst = 0;
    let sent = 0;
    for (let i = 1; i <= 60; i++) {
      const t0 = performance.now();
      const t = reduce(s, { type: "pointer-move", ...at(x0 + i * 3) });
      worst = Math.max(worst, performance.now() - t0);
      s = t.state;
      sent += updates(t.intents).length;
    }
    expect(worst).toBeLessThan(50);
    expect(sent).toBeLessThanOrEqual(60);
    expect(sent).toBeGreaterThan(0);
  });
});

describe("dropping media from the bin (WP-1M.03)", () => {
  const VIDEO = { id: "med_v", kind: "video", duration: sec(4) } as const;
  const SOUND = { id: "med_a", kind: "audio", duration: sec(3) } as const;
  const STILL = { id: "med_i", kind: "image", duration: null } as const;
  type Card = typeof VIDEO | typeof SOUND | typeof STILL;
  const over = (x: number, o: At = {}, media: Card = VIDEO) =>
    ({ type: "media-drag-over", media, ...pointer(x, o) }) as InteractionEvent;
  const drop = (x: number, o: At = {}, media: Card = VIDEO) =>
    ({ type: "media-drop", media, ...pointer(x, o) }) as InteractionEvent;
  const inserts = (intents: readonly Intent[]) =>
    intents.filter((i) => i.kind === "insert") as Extract<Intent, { kind: "insert" }>[];

  it("enters dropping-media over a lane and draws the drop guide", () => {
    const t = run(start(), over(xAt(sec(8)), { track: "v1" }));
    expect(t.state.mode.name).toBe("dropping-media");
    expect(isBusy(t.state)).toBe(true);
    expect(t.intents).toEqual([{ kind: "snap-guides", times: [sec(8)] }]);
  });

  it("drops a video on the track under the pointer, frame-aligned", () => {
    const x = xAt(sec(8)) + 1.4; // a little past 8 s: rounds to the frame grid
    const t = run(start(), over(x, { track: "v1" }), drop(x, { track: "v1" }));
    const [insert] = inserts(t.intents);
    expect(insert).toMatchObject({ media: "med_v", trackId: "v1" });
    expect((insert?.at ?? 0) % FRAME).toBe(0);
    expect(Math.abs((insert?.at ?? 0) - timeAt(x))).toBeLessThanOrEqual(FRAME / 2);
    expect(t.state.mode.name).toBe("idle");
    expect(t.intents.at(-1)).toEqual({ kind: "snap-guides", times: [] });
  });

  it("snaps the clip start to a clip edge within 8 px", () => {
    // B starts at 5 s; the pointer is 5 px (50 ms) before it.
    const x = xAt(sec(5)) - 5;
    const t = run(start(), drop(x, { track: "v2" }));
    expect(inserts(t.intents)[0]?.at).toBe(sec(5));
  });

  it("snaps the clip's end edge too: the span is [at, at + duration]", () => {
    // The 4 s clip dropped 4 px before 6 s would end 4 px before 10 s (the playhead).
    const x = xAt(sec(6)) - 4;
    const t = run(start(), drop(x, { track: "v2" }));
    expect(inserts(t.intents)[0]?.at).toBe(sec(6));
  });

  it("does not snap to edges when snapping is off; the frame grid still applies", () => {
    const x = xAt(sec(5)) - 5;
    const t = run(start({ snapping: false }), drop(x, { track: "v2" }));
    const at = inserts(t.intents)[0]?.at ?? -1;
    expect(at).not.toBe(sec(5));
    expect(at % FRAME).toBe(0);
  });

  it("puts sound on an audio track and pictures on a media track, whatever is under the pointer", () => {
    const sound = run(start(), drop(xAt(sec(8)), { track: "v1" }, SOUND));
    expect(inserts(sound.intents)[0]?.trackId).toBe("a1");
    const picture = run(start(), drop(xAt(sec(8)), { track: "a1" }, VIDEO));
    expect(inserts(picture.intents)[0]?.trackId).toBe("v2"); // the first (top) media track
    const still = run(start(), drop(xAt(sec(8)), { track: "a1" }, STILL));
    expect(inserts(still.intents)[0]?.trackId).toBe("v2");
  });

  it("falls back to the first unlocked track of the right kind", () => {
    const model: TimelineModel = {
      ...MODEL,
      tracks: [
        { id: "v2", name: "V2", kind: "media", locked: true },
        { id: "v1", name: "V1", kind: "media" },
        { id: "a1", name: "A1", kind: "audio" },
      ],
    };
    const t = run(start({ model }), drop(xAt(sec(8)), { track: "a1" }, VIDEO));
    expect(inserts(t.intents)[0]?.trackId).toBe("v1");
  });

  it("gives a still the 5 s default length for snapping its end", () => {
    // Dropped 3 px before 5 s it would end 3 px before 10 s (the playhead).
    const x = xAt(sec(5)) - 3;
    const model: TimelineModel = { ...MODEL, items: [] };
    const t = run(start({ model }), drop(x, { track: "v1" }, STILL));
    expect(inserts(t.intents)[0]?.at).toBe(sec(5));
  });

  it("has nowhere to drop when the sequence has no track of that kind", () => {
    const model: TimelineModel = {
      ...MODEL,
      tracks: [{ id: "a1", name: "A1", kind: "audio" }],
      items: [],
    };
    const t = run(
      start({ model }),
      over(xAt(sec(2)), { track: "a1" }),
      drop(xAt(sec(2)), { track: "a1" }),
    );
    expect(inserts(t.intents)).toEqual([]);
    expect(t.state.mode.name).toBe("idle");
  });

  it("clamps a drop over the header (negative time) to the timeline start", () => {
    const t = run(start(), drop(HEADER - 60, { track: "v1", zone: "header" }));
    expect(inserts(t.intents)[0]?.at).toBe(0);
  });

  it("repeats nothing while the snapped position and track are unchanged", () => {
    const x = xAt(sec(8));
    const first = reduce(start(), over(x, { track: "v1" }));
    const again = reduce(first.state, over(x + 0.2, { track: "v1" }));
    expect(again.intents).toEqual([]);
    expect(again.state).toBe(first.state);
  });

  it("moves the guide when the card moves to another frame", () => {
    const first = reduce(start(), over(xAt(sec(8)), { track: "v1" }));
    const next = reduce(first.state, over(xAt(sec(8)) + 30, { track: "v1" }));
    expect(next.intents).toHaveLength(1);
    const guide = next.intents[0] as Extract<Intent, { kind: "snap-guides" }>;
    expect(guide.times[0]).toBeGreaterThan(sec(8));
  });

  it("leaving the canvas clears the preview without inserting", () => {
    const t = run(start(), over(xAt(sec(8)), { track: "v1" }), { type: "media-drag-leave" });
    expect(t.state.mode.name).toBe("idle");
    expect(inserts(t.intents)).toEqual([]);
    expect(t.intents.at(-1)).toEqual({ kind: "snap-guides", times: [] });
  });

  it("Esc cancels the drop; a later leave has nothing to clear", () => {
    const t = run(start(), over(xAt(sec(8)), { track: "v1" }), key("Escape"));
    expect(t.state.mode.name).toBe("idle");
    expect(inserts(t.intents)).toEqual([]);
    expect(reduce(t.state, { type: "media-drag-leave" }).intents).toEqual([]);
  });

  it("ignores card drags while a clip is being dragged", () => {
    const s = run(
      start(),
      down(xAt(sec(2)), { item: A }),
      move(xAt(sec(2)) + 20, { item: A }),
    ).state;
    expect(isGestureOpen(s)).toBe(true);
    expect(reduce(s, over(xAt(sec(8)), { track: "v1" })).state).toBe(s);
    expect(reduce(s, drop(xAt(sec(8)), { track: "v1" })).intents).toEqual([]);
  });

  it("keys and pointer moves are inert while dropping", () => {
    const s = run(start(), over(xAt(sec(8)), { track: "v1" })).state;
    expect(reduce(s, key("s")).intents).toEqual([]);
    expect(reduce(s, move(xAt(sec(9)), { track: "v1" })).state).toBe(s);
  });
});
