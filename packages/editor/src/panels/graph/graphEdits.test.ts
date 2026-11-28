// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.07: the graph editor's curves as a projection of the document, and its edits as the
// engine's existing keyframe commands (`keyframe.move`, `keyframe.set`, `keyframe.set_interp`).
import type { Project } from "@cutweave/protocol";
import type { GraphEdit } from "@cutweave/ui";
import { describe, expect, it } from "vitest";
import { sampleProject, sec } from "../../testing/fixtures.ts";
import type { KeySelection } from "../../timeline/keyframes.ts";
import {
  type EditOrigin,
  type MoveOutput,
  moveStep,
  nudgeStep,
  tangentCommands,
  tangentStep,
  valueStep,
} from "./graphEdits.ts";
import type { StepSend } from "./graphGesture.ts";
import {
  buildCurves,
  CompiledCache,
  type CurveInputs,
  clampToProp,
  curveId,
  keyOfPoint,
  pointsOf,
  sameClips,
  selectedClips,
} from "./graphModel.ts";
import { keyedProject } from "./testing.ts";

const inputs = (project: Project, more: Partial<CurveInputs> = {}): CurveInputs => ({
  project,
  sequence: "seq_1" as never,
  items: ["itm_v"],
  hidden: [],
  label: (prop) => prop,
  clipLabel: (item) => item,
  ...more,
});

const build = (project = keyedProject(), more: Partial<CurveInputs> = {}) =>
  buildCurves(inputs(project, more), new CompiledCache());

const X = curveId("itm_v", "transform.position", 0);
const Y = curveId("itm_v", "transform.position", 1);
const OPACITY = curveId("itm_v", "opacity", 0);

const selection = (keys: KeySelection["keys"], offset = 0): KeySelection => ({
  item: "itm_v",
  keys,
  offset,
});

/** A `send` that writes the commands down and answers `keyframe.move` as the engine does. */
function recorder() {
  const sent: { name: string; args: Record<string, unknown> }[] = [];
  const send: StepSend = async (name, args) => {
    const a = args as Record<string, unknown>;
    sent.push({ name, args: a });
    if (name !== "keyframe.move") return {};
    const keys = a.keys as { prop: string; t: number }[];
    const delta = a.delta as number;
    return { moved: keys.map((k) => ({ prop: k.prop, from_t: k.t, t: k.t + delta })) };
  };
  return { sent, send };
}

describe("the curves of the selected clips", () => {
  it("is one curve per animated property, two for a position, in sequence time", () => {
    const built = build();
    expect(built.curves.map((c) => c.id)).toEqual([X, Y, OPACITY]);
    expect(built.curves.map((c) => c.label)).toEqual([
      "transform.position X",
      "transform.position Y",
      "opacity",
    ]);
    const x = built.curves[0];
    // The clip starts at 1 s: its keys at 0, 1 and 2 s of item time are at 1, 2 and 3 s.
    expect(Array.from(x?.times ?? [])).toEqual([sec(1), sec(2), sec(3)]);
    expect(Array.from(x?.itemTimes ?? [])).toEqual([0, sec(1), sec(2)]);
    expect(Array.from(x?.values ?? [])).toEqual([-400, 200, 0]);
    expect(Array.from(built.curves[1]?.values ?? [])).toEqual([0, -150, 300]);
    expect(x?.interps[0]).toMatchObject({ kind: "bezier", x1: 0.42 });
    expect(built.curves.map((c) => c.color)).toEqual([0, 1, 2]);
    expect(built.groups.map((g) => [g.prop, g.colors, g.hidden])).toEqual([
      ["transform.position", [0, 1], false],
      ["opacity", [2], false],
    ]);
  });

  it("leaves out a hidden property's curves and still lists it", () => {
    const built = build(keyedProject(), { hidden: ["transform.position"] });
    expect(built.curves.map((c) => c.id)).toEqual([OPACITY]);
    expect(built.groups.map((g) => [g.prop, g.hidden])).toEqual([
      ["transform.position", true],
      ["opacity", false],
    ]);
    // The opacity curve keeps the colour it has while the position is shown.
    expect(built.curves[0]?.color).toBe(2);
  });

  it("shows nothing for a clip without animation, a clip of another sequence or no selection", () => {
    expect(build(sampleProject()).curves).toEqual([]);
    expect(build(keyedProject(), { items: [] }).curves).toEqual([]);
    expect(build(keyedProject(), { items: ["itm_missing"] }).curves).toEqual([]);
    expect(build(keyedProject(), { sequence: null }).curves).toEqual([]);
  });

  it("knows when the selected clips are the same objects: nothing to rebuild", () => {
    const project = keyedProject();
    const built = build(project);
    expect(sameClips(built, selectedClips(project, "seq_1" as never, ["itm_v"]))).toBe(true);
    expect(sameClips(built, selectedClips(keyedProject(), "seq_1" as never, ["itm_v"]))).toBe(
      false,
    );
    expect(sameClips(built, [])).toBe(false);
  });

  it("maps the selected keys to points, through the component mask and a drag's offset", () => {
    const { curves } = build();
    const key = { prop: "transform.position", t: sec(1) };
    expect(pointsOf(curves, selection([key]), null)).toEqual([
      { curve: X, index: 1 },
      { curve: Y, index: 1 },
    ]);
    expect(pointsOf(curves, selection([key]), ["transform.position|1"])).toEqual([
      { curve: Y, index: 1 },
    ]);
    // Dragged one second later in the document already: still the selected key.
    const dragged = selection([{ prop: "transform.position", t: 0 }], sec(1));
    expect(pointsOf(curves, dragged, null)).toEqual([
      { curve: X, index: 1 },
      { curve: Y, index: 1 },
    ]);
    expect(pointsOf(curves, { item: "itm_a", keys: [key], offset: 0 }, null)).toEqual([]);
    expect(keyOfPoint(curves, { curve: Y, index: 2 })?.key).toEqual({
      prop: "transform.position",
      t: sec(2),
    });
  });

  it("keeps a value inside its property's stored range", () => {
    expect(clampToProp("opacity", 1.4)).toBe(1);
    expect(clampToProp("opacity", -0.1)).toBe(0);
    expect(clampToProp("transform.scale", -2)).toBe(0);
    expect(clampToProp("transform.position", -5000)).toBe(-5000);
    expect(clampToProp("audio_gain", 100)).toBe(48);
  });
});

describe("edits as engine commands", () => {
  const origin = (keys: EditOrigin["keys"]): EditOrigin => ({
    item: "itm_v",
    keys,
    curves: build().curves,
  });
  const move = (dt: number, dv: Record<string, number>): Extract<GraphEdit, { type: "move" }> => ({
    type: "move",
    grabbed: { curve: X, index: 1 },
    dt,
    dv,
  });
  const middle = { prop: "transform.position", t: sec(1) };

  it("moves keys in time with keyframe.move and in value with keyframe.set at their new time", async () => {
    const { sent, send } = recorder();
    const step = moveStep(origin([middle]), move(sec(0.5), { [X]: 50 }));
    const out = (await step?.run(send)) as MoveOutput;
    expect(sent).toEqual([
      {
        name: "keyframe.move",
        args: { item: "itm_v", keys: [middle], delta: sec(0.5) },
      },
      {
        name: "keyframe.set",
        // Only X changed: Y keeps its stored value. No easing given: the key keeps its own.
        args: {
          item: "itm_v",
          prop: "transform.position",
          keys: [{ t: sec(1.5), value: { x: 250, y: -150 } }],
        },
      },
    ]);
    expect(out).toEqual({ keys: [{ prop: "transform.position", t: sec(1.5) }], shift: sec(0.5) });
  });

  it("sends only what changed: time alone, or value alone", async () => {
    const time = recorder();
    await moveStep(origin([middle]), move(sec(1) / 30, {}))?.run(time.send);
    expect(time.sent.map((c) => c.name)).toEqual(["keyframe.move"]);
    const value = recorder();
    await moveStep(origin([middle]), move(0, { [X]: -20, [Y]: 10 }))?.run(value.send);
    expect(value.sent).toEqual([
      {
        name: "keyframe.set",
        args: {
          item: "itm_v",
          prop: "transform.position",
          keys: [{ t: sec(1), value: { x: 180, y: -140 } }],
        },
      },
    ]);
  });

  it("moves keys of several properties together and stops a value at its range", async () => {
    const { sent, send } = recorder();
    const keys = [middle, { prop: "opacity", t: sec(2) }];
    await moveStep(origin(keys), move(0, { [X]: 10, [OPACITY]: 0.5 }))?.run(send);
    expect(sent.map((c) => [c.name, c.args.prop, c.args.keys])).toEqual([
      ["keyframe.set", "transform.position", [{ t: sec(1), value: { x: 210, y: -150 } }]],
      ["keyframe.set", "opacity", [{ t: sec(2), value: 1 }]],
    ]);
  });

  it("puts the keys back when the pointer returns to where the drag began", async () => {
    const { sent, send } = recorder();
    const step = moveStep(origin([middle]), move(0, {}));
    expect(step?.key).toBe("rest");
    await step?.run(send);
    // One command that changes nothing: with `replace` it rolls the earlier steps back.
    expect(sent).toEqual([
      {
        name: "keyframe.set",
        args: {
          item: "itm_v",
          prop: "transform.position",
          keys: [{ t: sec(1), value: { x: 200, y: -150 } }],
        },
      },
    ]);
    expect(moveStep(origin([]), move(sec(1), {}))).toBeNull();
  });

  it("writes a dragged out handle as (x1, y1) of the key's own segment", () => {
    const edit: GraphEdit = {
      type: "tangent",
      curve: X,
      index: 1,
      side: "out",
      x: 0.25,
      y: 0.5,
      opposite: null,
    };
    expect(tangentCommands(origin([middle]), edit)).toEqual([
      {
        name: "keyframe.set_interp",
        args: {
          item: "itm_v",
          keys: [{ prop: "transform.position", t: sec(1) }],
          // The numbers alone: the `ease_in_out` hint is gone with the preset's shape.
          interp: { kind: "bezier", x1: 0.25, y1: 0.5, x2: 0.58, y2: 1 },
        },
      },
    ]);
  });

  it("writes a dragged in handle as (x2, y2) of the segment before the key, and the tied handle across it", () => {
    const edit: GraphEdit = {
      type: "tangent",
      curve: Y,
      index: 1,
      side: "in",
      x: 0.7,
      y: 0.9,
      opposite: { x: 0.3, y: -0.2 },
    };
    const commands = tangentCommands(origin([middle]), edit);
    expect(commands.map((c) => [c.args.keys, c.args.interp])).toEqual([
      [
        [{ prop: "transform.position", t: 0 }],
        { kind: "bezier", x1: 0.42, y1: 0, x2: 0.7, y2: 0.9 },
      ],
      [
        [{ prop: "transform.position", t: sec(1) }],
        { kind: "bezier", x1: 0.3, y1: -0.2, x2: 0.58, y2: 1 },
      ],
    ]);
    expect(tangentStep(origin([middle]), edit)?.key).toContain("tangent:in:0.7:0.9");
  });

  it("has no handle to write on a segment that is not a bezier", () => {
    const edit: GraphEdit = {
      type: "tangent",
      curve: X,
      index: 2,
      side: "out",
      x: 0.2,
      y: 0.2,
      opposite: null,
    };
    expect(tangentCommands(origin([middle]), edit)).toEqual([]);
    expect(tangentStep(origin([middle]), edit)).toBeNull();
  });

  it("nudges by a time or by value steps, and sets a typed value, on the masked component", async () => {
    const onlyY = (c: { comp: number; prop: string }) =>
      c.prop === "transform.position" && c.comp === 1;
    const time = recorder();
    await nudgeStep(origin([middle]), -sec(1) / 30, () => 0, onlyY)?.run(time.send);
    expect(time.sent).toEqual([
      { name: "keyframe.move", args: { item: "itm_v", keys: [middle], delta: -sec(1) / 30 } },
    ]);
    const value = recorder();
    await nudgeStep(origin([middle]), 0, () => 10, onlyY)?.run(value.send);
    expect(value.sent[0]?.args.keys).toEqual([{ t: sec(1), value: { x: 200, y: -140 } }]);
    const typed = recorder();
    await valueStep(origin([middle]), 42, onlyY)?.run(typed.send);
    expect(typed.sent[0]?.args.keys).toEqual([{ t: sec(1), value: { x: 200, y: 42 } }]);
    expect(nudgeStep(origin([middle]), 0, () => 0, onlyY)).toBeNull();
  });
});
