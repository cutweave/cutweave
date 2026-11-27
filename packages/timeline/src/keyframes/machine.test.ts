// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.05: the interaction machine on keyframe lanes. A key dragged in a lane is a
// `keyframe_dragging` gesture whose updates name the lane's property (one transaction, the last
// update kept on release); Shift or Ctrl on a lane key leaves the clips alone (the host adds the
// key to its selection); the empty part of a lane starts nothing.
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath } from "@cutweave/protocol/wasm";
import { beforeAll, describe, expect, it } from "vitest";
import { f, HEADER, PX_PER_FRAME, World } from "../interaction/testing/world.ts";
import { testTheme } from "../test-support.ts";
import { type ItemLanes, laneRowHeight } from "./lanes.ts";

beforeAll(async () => {
  await initMath(mathModuleBytes());
}, 900_000);

const th = testTheme();
const ROW = laneRowHeight(th);
/** Y sits on V2, the third track; its lanes open under it. */
const LANES: readonly ItemLanes[] = [
  {
    itemId: "Y",
    lanes: [
      { prop: "opacity", label: "Opacity", times: [f(50), f(70)], glyphs: [0, 0] },
      { prop: "transform.rotation", label: "Rotation", times: [f(60)], glyphs: [0] },
    ],
  },
];
const x = (frame: number): number => HEADER + frame * PX_PER_FRAME;
/** Center of lane `row` of Y: under two md tracks and V2's clip. */
const laneY = (row: number): number => {
  const track = th.trackMd + 2 * th.space1;
  return th.controlMd + 2 * track + th.space1 + th.trackMd + row * ROW + ROW / 2;
};

function world(): World {
  return new World({ keyframes: true, lanes: LANES });
}

describe("dragging a key in a keyframe lane", () => {
  it("is one gesture whose updates move the lane's property key", () => {
    const w = world();
    expect(w.deps.hitTest(x(60), laneY(1)).zone).toBe("keyframe");
    w.send({ type: "pointer.down", x: x(60), y: laneY(1), button: 0 });
    expect(w.state.mode.name).toBe("pressed");
    w.send({ type: "pointer.move", x: x(60) + 3 * PX_PER_FRAME + 8, y: laneY(1) });
    expect(w.state.mode.name).toBe("keyframe_dragging");
    w.send({ type: "frame" });
    w.send({ type: "pointer.move", x: x(66), y: laneY(1) });
    w.send({ type: "frame" });
    w.send({ type: "pointer.up", x: x(66), y: laneY(1), button: 0 });
    expect(w.engine.begins).toBe(1);
    expect(w.engine.closes).toBe(1);
    const last = w.engine.history.at(-1);
    expect(last?.name).toBe("keyframe.move");
    expect(last?.args).toMatchObject({
      item: "Y",
      keys: [{ prop: "transform.rotation", t: f(60) }],
      to_at: f(66),
    });
    // The press selected the key's clip.
    expect(w.state.env.selection).toEqual(["Y"]);
  });

  it("snaps the key to the playhead", () => {
    const w = new World({ keyframes: true, lanes: LANES, env: { playhead: f(55) } });
    w.send({ type: "pointer.down", x: x(50), y: laneY(0), button: 0 });
    w.send({ type: "pointer.move", x: x(54) + 2, y: laneY(0) });
    w.send({ type: "frame" });
    w.send({ type: "pointer.up", x: x(54) + 2, y: laneY(0), button: 0 });
    expect(w.engine.history.at(-1)?.args).toMatchObject({
      keys: [{ prop: "opacity", t: f(50) }],
      to_at: f(55),
    });
  });

  it("Esc rolls the drag back", () => {
    const w = world();
    w.send({ type: "pointer.down", x: x(50), y: laneY(0), button: 0 });
    w.send({ type: "pointer.move", x: x(58), y: laneY(0) });
    w.send({ type: "frame" });
    w.send({ type: "key.down", key: "Escape" });
    expect(w.engine.history).toHaveLength(0);
    expect(w.engine.closes).toBe(1);
  });
});

describe("modifiers and the empty lane", () => {
  it("Shift or Ctrl on a lane key changes neither the clip selection nor the state", () => {
    for (const mod of [{ shift: true }, { ctrl: true }]) {
      const w = world();
      w.send({ type: "pointer.down", x: x(10), y: laneY(0) - ROW - 10, button: 0 });
      w.send({ type: "pointer.up", x: x(10), y: laneY(0) - ROW - 10, button: 0 });
      const before = w.state.env.selection;
      w.send({ type: "pointer.down", x: x(50), y: laneY(0), button: 0, ...mod });
      expect(w.state.mode.name).not.toBe("pressed");
      expect(w.state.env.selection).toEqual(before);
      expect(w.engine.begins).toBe(0);
    }
  });

  it("a press on the empty part of a lane is the host's: no marquee, no gesture", () => {
    const w = world();
    expect(w.deps.hitTest(x(65), laneY(0)).zone).toBe("keyframe-lane");
    w.send({ type: "pointer.down", x: x(65), y: laneY(0), button: 0 });
    w.send({ type: "pointer.move", x: x(75), y: laneY(0) });
    expect(["idle", "hover"]).toContain(w.state.mode.name);
    expect(w.engine.begins).toBe(0);
  });
});
