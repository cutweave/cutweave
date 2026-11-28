// SPDX-License-Identifier: GPL-3.0-or-later
// Issue #262: a drag pressed inside an auto-scroll zone must not scroll the timeline until the
// pointer has moved toward that edge. In the editor's default layout the timeline shows about
// three lanes, and the third lane's centre is 20 px from the bottom edge: a sideways drag of a
// clip there scrolled the tracks under the pointer and put the clip on another track.
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath } from "@cutweave/protocol/wasm";
import { beforeAll, describe, expect, it } from "vitest";
import { AUTO_SCROLL_EDGE_PX, AUTO_SCROLL_INTENT_PX } from "./constants.ts";
import { clip, fmt, fmtCommand } from "./testing/transcript.ts";
import { World } from "./testing/world.ts";

beforeAll(async () => {
  await initMath(mathModuleBytes());
}, 900_000);

/** A world whose lane area ends `below` px under the middle of `track` (the rest scrolls). */
function worldEndingBelow(track: string, below: number): World {
  const w = new World();
  w.vp.height = w.yOf(track) + below;
  return w;
}

const scrolls = (w: World): string[] => w.intents.filter((i) => i.type === "scroll_by").map(fmt);

const toTracks = (w: World): unknown[] =>
  w.intents.flatMap((i) =>
    i.type === "update_tx" && i.command.name === "item.move"
      ? [(i.command.args as { to_track?: string }).to_track]
      : [],
  );

/** Presses at `from`, then moves to each of `path` with a few frames after each. */
function dragAlong(
  w: World,
  from: { x: number; y: number },
  path: readonly { x: number; y: number }[],
): void {
  w.send({ type: "pointer.down", ...from, button: 0 });
  for (const p of path) {
    w.send({ type: "pointer.move", ...p });
    for (let i = 0; i < 4; i++) w.send({ type: "frame" });
  }
}

describe("auto-scroll needs the pointer to move toward the edge (#262)", () => {
  it("a sideways drag in the bottom lane, inside the zone, keeps its track and scrolls nothing", () => {
    const w = worldEndingBelow("V2", 20);
    const z = clip("Z")(w);
    expect(w.vp.height - z.y).toBeLessThan(AUTO_SCROLL_EDGE_PX);
    const path = [];
    for (let dx = 4; dx <= 200; dx += 14) path.push({ x: z.x + dx, y: z.y });
    dragAlong(w, z, path);
    expect(w.state.mode.name).toBe("moving");
    expect(scrolls(w)).toEqual([]);
    expect(w.vp.scrollY).toBe(0);
    expect(w.state.mode.name === "moving" && w.state.mode.toTrack).toBe("V2");
    w.send({ type: "pointer.up", x: z.x + 200, y: z.y });
    // Every update stayed on the home track (no `to_track`), and so does the undo step.
    expect(toTracks(w).length).toBeGreaterThan(0);
    expect(toTracks(w).every((t) => t === undefined)).toBe(true);
    expect(w.engine.history.map(fmtCommand)).toHaveLength(1);
    expect(w.engine.history.map(fmtCommand)[0]).toMatch(
      /^item\.move items=\[Z\] start=\d+f mode=new_track$/,
    );
  });

  it("a hand's wobble under the intent distance scrolls nothing either", () => {
    const w = worldEndingBelow("V2", 20);
    const z = clip("Z")(w);
    const wobble = AUTO_SCROLL_INTENT_PX - 1;
    dragAlong(w, z, [
      { x: z.x + 30, y: z.y + wobble },
      { x: z.x + 60, y: z.y - wobble },
      { x: z.x + 90, y: z.y + wobble },
    ]);
    expect(scrolls(w)).toEqual([]);
    expect(w.state.mode.name === "moving" && w.state.mode.toTrack).toBe("V2");
  });

  it("moving toward the bottom edge from a press inside the zone does scroll, and the clip changes track", () => {
    const w = worldEndingBelow("V2", 20);
    const z = clip("Z")(w);
    dragAlong(w, z, [{ x: z.x + 30, y: z.y + AUTO_SCROLL_INTENT_PX + 2 }]);
    expect(scrolls(w).length).toBeGreaterThan(0);
    expect(w.vp.scrollY).toBeGreaterThan(0);
    // The tracks came up under the still pointer: the main track is under it now.
    expect(w.state.mode.name === "moving" && w.state.mode.toTrack).toBe("V1");
  });

  it("a press outside the zone scrolls as soon as the pointer enters it", () => {
    const w = worldEndingBelow("V2", AUTO_SCROLL_EDGE_PX + 60);
    const z = clip("Z")(w);
    dragAlong(w, z, [{ x: z.x + 30, y: w.vp.height - AUTO_SCROLL_EDGE_PX + 2 }]);
    expect(scrolls(w).length).toBeGreaterThan(0);
  });

  it("the top edge: a sideways drag inside the zone of a scrolled timeline does not scroll up", () => {
    const w = new World({ env: { scrollY: 110 } });
    const z = clip("Z")(w);
    expect(z.y - w.vp.rulerHeight).toBeLessThan(AUTO_SCROLL_EDGE_PX);
    dragAlong(w, z, [
      { x: z.x + 40, y: z.y },
      { x: z.x + 120, y: z.y },
    ]);
    expect(scrolls(w)).toEqual([]);
    expect(w.vp.scrollY).toBe(110);
    // Toward the edge: it scrolls.
    w.send({ type: "pointer.move", x: z.x + 120, y: z.y - AUTO_SCROLL_INTENT_PX - 2 });
    w.send({ type: "frame" });
    expect(scrolls(w).length).toBeGreaterThan(0);
  });

  it("the right edge: a clip pressed inside the zone and moved only up or down keeps its time", () => {
    const w = new World();
    // Z's body sits under the right edge's zone.
    w.vp.width = clip("Z")(w).x + 10;
    const z = clip("Z")(w);
    dragAlong(w, z, [
      { x: z.x, y: z.y - 8 },
      { x: z.x, y: z.y + 8 },
    ]);
    expect(w.state.mode.name).toBe("moving");
    expect(scrolls(w)).toEqual([]);
    expect(w.vp.scrollX).toBe(0);
    w.send({ type: "pointer.up", x: z.x, y: z.y + 8 });
    expect(w.engine.history).toEqual([]);
  });

  it("a trim pressed inside the right edge's zone does not scroll until the pointer moves right", () => {
    const w = new World();
    const end = clip("Z", "end")(w);
    w.vp.width = end.x + 10;
    dragAlong(w, end, [
      { x: end.x, y: end.y + 6 },
      { x: end.x - 2, y: end.y - 6 },
    ]);
    expect(w.state.mode.name).toBe("trimming");
    expect(scrolls(w)).toEqual([]);
    w.send({ type: "pointer.move", x: end.x + AUTO_SCROLL_INTENT_PX + 2, y: end.y });
    w.send({ type: "frame" });
    expect(scrolls(w).length).toBeGreaterThan(0);
  });

  it("a keyframe pressed inside the left edge's zone does not scroll back until the pointer moves left", () => {
    const w = new World({ keyframes: true, env: { scrollX: 0 } });
    // Scroll so Y's first key (frame 50) is 10 px into the lanes.
    const k0 = clip("Y", 0)(w);
    const shift = k0.x - (w.vp.headerWidth + 10);
    w.send({ type: "pointer.down", x: 500, y: 300, button: 1 });
    w.send({ type: "pointer.move", x: 500 - shift, y: 300 });
    w.send({ type: "pointer.up", x: 500 - shift, y: 300, button: 1 });
    expect(w.vp.scrollX).toBeGreaterThan(0);
    const k = clip("Y", 0)(w);
    expect(k.x - w.vp.headerWidth).toBeLessThan(AUTO_SCROLL_EDGE_PX);
    const before = w.vp.scrollX;
    dragAlong(w, k, [
      { x: k.x + 4, y: k.y },
      { x: k.x + 1, y: k.y },
    ]);
    expect(w.state.mode.name).toBe("keyframe_dragging");
    expect(scrolls(w)).toEqual([]);
    expect(w.vp.scrollX).toBe(before);
  });
});
