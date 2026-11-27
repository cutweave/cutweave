// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.05: the editor's side of keyframe lanes. The lanes are a projection of the document
// (sequence times, a glyph per interpolation, selected keys marked, also while they are
// dragged); the key selection holds keys of one clip; every edit is one engine command.
import type { Clip, KeyframeClipboard, Project, SequenceId } from "@cutweave/protocol";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  addKey,
  animatedProps,
  clearKeys,
  createLanesToggle,
  deleteKeys,
  EMPTY_KEY_SELECTION,
  keySelectionStore,
  keysAt,
  laneGroups,
  lanesLaidStore,
  lanesRelaid,
  lanesShapeOf,
  moveArgs,
  movedKeys,
  NO_GROUPS,
  nudgeKeys,
  orderedKeys,
  parseKeyPayload,
  pasteKeys,
  pruneKeys,
  selectKeys,
  setEasing,
  toggleKey,
} from "./keyframes.ts";

const key = (t: number, kind: "linear" | "hold" | "bezier" | "spring" = "linear") => ({
  t,
  value: 1,
  interp:
    kind === "bezier"
      ? { kind, x1: 0.42, y1: 0, x2: 0.58, y2: 1 }
      : kind === "spring"
        ? { kind, stiffness: 170, damping: 26, mass: 1, bounce: false }
        : { kind },
});

/** A clip at 100 with animated opacity (keys at 0, 10, 20) and rotation (a key at 10). */
const CLIP = {
  id: "itm_a",
  start: 100,
  duration: 50,
  visual: {
    opacity: { keyframes: [key(0), key(10, "hold"), key(20, "bezier")] },
    transform: {
      position: { x: 0, y: 0 },
      scale: { x: 1, y: 1 },
      rotation: { keyframes: [key(10, "spring")] },
      anchor: { x: 0.5, y: 0.5 },
    },
  },
  audio: { gain: 0, pan: 0 },
} as unknown as Clip;

const STATIC = {
  id: "itm_b",
  start: 0,
  duration: 50,
  visual: { opacity: 1, transform: { rotation: 0 } },
  audio: { gain: 0, pan: 0 },
} as unknown as Clip;

const SEQ = "seq_1" as SequenceId;
const PROJECT = {
  sequences: {
    [SEQ]: { tracks: { trk_1: { items: { itm_a: CLIP, itm_b: STATIC } } } },
  },
} as unknown as Project;

const label = (prop: string) => `L:${prop}`;

beforeEach(() => clearKeys());

describe("the lanes of open clips", () => {
  it("lists a clip's animated properties in lane order, at sequence times, with glyphs", () => {
    expect(animatedProps(CLIP)).toEqual(["transform.rotation", "opacity"]);
    const groups = laneGroups(PROJECT, SEQ, ["itm_a"], EMPTY_KEY_SELECTION, label);
    expect(groups).toHaveLength(1);
    const lanes = groups[0]?.lanes ?? [];
    expect(lanes.map((l) => l.prop)).toEqual(["transform.rotation", "opacity"]);
    expect(lanes[1]?.label).toBe("L:opacity");
    expect(Array.from(lanes[1]?.times ?? [])).toEqual([100, 110, 120]);
    // linear, hold, bezier; the rotation key is a spring.
    expect(Array.from(lanes[1]?.glyphs ?? [])).toEqual([0, 1, 2]);
    expect(Array.from(lanes[0]?.glyphs ?? [])).toEqual([3]);
  });

  it("leaves out clips with no animated property, unknown clips and closed ones", () => {
    expect(laneGroups(PROJECT, SEQ, ["itm_b", "gone"], EMPTY_KEY_SELECTION, label)).toHaveLength(0);
    expect(laneGroups(PROJECT, SEQ, [], EMPTY_KEY_SELECTION, label)).toHaveLength(0);
    expect(laneGroups(null, null, ["itm_a"], EMPTY_KEY_SELECTION, label)).toHaveLength(0);
  });

  it("marks the selected keys, and keeps them marked while a drag has moved them", () => {
    const selection = { item: "itm_a", keys: [{ prop: "opacity", t: 10 }], offset: 0 };
    const lanes = laneGroups(PROJECT, SEQ, ["itm_a"], selection, label)[0]?.lanes ?? [];
    expect(lanes[1]?.selected).toEqual([1]);
    expect(lanes[0]?.selected).toBeUndefined();
    // Dragged 10 flicks later: the document has the key at 20 now.
    const dragged = { item: "itm_a", keys: [{ prop: "opacity", t: 10 }], offset: 10 };
    const during = laneGroups(PROJECT, SEQ, ["itm_a"], dragged, label)[0]?.lanes ?? [];
    expect(during[1]?.selected).toEqual([2]);
  });

  it("reuses a clip's arrays while the clip object is the same", () => {
    const a = laneGroups(PROJECT, SEQ, ["itm_a"], EMPTY_KEY_SELECTION, label)[0]?.lanes[0];
    const b = laneGroups(PROJECT, SEQ, ["itm_a"], EMPTY_KEY_SELECTION, label)[0]?.lanes[0];
    expect(a?.times).toBe(b?.times);
  });
});

describe("the key selection", () => {
  it("toggles keys of one clip and starts over on another clip", () => {
    toggleKey("itm_a", { prop: "opacity", t: 0 });
    toggleKey("itm_a", { prop: "opacity", t: 10 });
    expect(keySelectionStore.getState().keys).toHaveLength(2);
    toggleKey("itm_a", { prop: "opacity", t: 0 });
    expect(keySelectionStore.getState().keys).toEqual([{ prop: "opacity", t: 10 }]);
    toggleKey("itm_b", { prop: "opacity", t: 5 });
    expect(keySelectionStore.getState()).toMatchObject({
      item: "itm_b",
      keys: [{ prop: "opacity", t: 5 }],
    });
    toggleKey("itm_b", { prop: "opacity", t: 5 });
    expect(keySelectionStore.getState().item).toBeNull();
  });

  it("drops keys the document no longer has", () => {
    selectKeys("itm_a", [
      { prop: "opacity", t: 10 },
      { prop: "opacity", t: 15 },
    ]);
    pruneKeys(PROJECT);
    expect(keySelectionStore.getState().keys).toEqual([{ prop: "opacity", t: 10 }]);
    selectKeys("gone", [{ prop: "opacity", t: 10 }]);
    pruneKeys(PROJECT);
    expect(keySelectionStore.getState().item).toBeNull();
  });

  it("orders a clip's keys by time, then lane, and finds every property's key at a time", () => {
    expect(orderedKeys(CLIP).map((k) => `${k.prop}@${k.at}`)).toEqual([
      "opacity@100",
      "transform.rotation@110",
      "opacity@110",
      "opacity@120",
    ]);
    expect(keysAt(CLIP, 10)).toEqual([
      { prop: "transform.rotation", t: 10 },
      { prop: "opacity", t: 10 },
    ]);
  });
});

describe("edits are one command each", () => {
  it("a drag moves one key to a sequence time, several by a distance", () => {
    expect(moveArgs("itm_a", [{ prop: "opacity", t: 10 }], 110, 130)).toEqual({
      item: "itm_a",
      keys: [{ prop: "opacity", t: 10 }],
      to_at: 130,
    });
    const many = moveArgs(
      "itm_a",
      [
        { prop: "opacity", t: 10 },
        { prop: "transform.rotation", t: 10 },
      ],
      110,
      90,
    );
    expect(many.delta).toBe(-20);
    expect(many.to_at).toBeUndefined();
  });

  it("nudge sends keyframe.move and keeps the moved keys selected", async () => {
    const call = vi.fn(async () => ({ moved: [{ prop: "opacity", from_t: 10, t: 12, at: 112 }] }));
    await nudgeKeys(call, "itm_a", [{ prop: "opacity", t: 10 }], 2);
    expect(call).toHaveBeenCalledTimes(1);
    expect(call).toHaveBeenCalledWith("keyframe.move", {
      item: "itm_a",
      keys: [{ prop: "opacity", t: 10 }],
      delta: 2,
    });
    expect(keySelectionStore.getState().keys).toEqual([{ prop: "opacity", t: 12 }]);
    expect(movedKeys(null)).toEqual([]);
  });

  it("delete sends keyframe.delete and lets go of the keys", async () => {
    selectKeys("itm_a", [{ prop: "opacity", t: 10 }]);
    const call = vi.fn(async () => ({}));
    await deleteKeys(call, "itm_a", [{ prop: "opacity", t: 10 }]);
    expect(call).toHaveBeenCalledWith("keyframe.delete", {
      item: "itm_a",
      keys: [{ prop: "opacity", t: 10 }],
    });
    expect(keySelectionStore.getState().item).toBeNull();
  });

  it("add sends keyframe.set with no value (the property's value there) and selects the key", async () => {
    const call = vi.fn(async () => ({ keys: [{ item: "itm_a", prop: "opacity", t: 30 }] }));
    await addKey(call, "itm_a", "opacity", 130);
    expect(call).toHaveBeenCalledWith("keyframe.set", {
      item: "itm_a",
      prop: "opacity",
      keys: [{ at: 130 }],
    });
    expect(keySelectionStore.getState().keys).toEqual([{ prop: "opacity", t: 30 }]);
  });

  it("easing sends keyframe.set_interp with a preset name or the stored form", async () => {
    const call = vi.fn(async () => ({}));
    await setEasing(call, "itm_a", [{ prop: "opacity", t: 10 }], { easing: "ease_in_out" });
    expect(call).toHaveBeenLastCalledWith("keyframe.set_interp", {
      item: "itm_a",
      keys: [{ prop: "opacity", t: 10 }],
      easing: "ease_in_out",
    });
    await setEasing(call, "itm_a", [{ prop: "opacity", t: 10 }], { interp: { kind: "hold" } });
    expect(call).toHaveBeenLastCalledWith(
      "keyframe.set_interp",
      expect.objectContaining({ interp: { kind: "hold" } }),
    );
  });

  it("paste sends keyframe.paste at the playhead and selects what it wrote", async () => {
    const payload = {
      version: 1,
      keys: [{ prop: "opacity", dt: 0 }],
    } as never as KeyframeClipboard;
    const call = vi.fn(async () => ({
      keys: [{ item: "itm_a", prop: "opacity", t: 40, at: 140 }],
    }));
    await pasteKeys(call, payload, "itm_a", 140);
    expect(call).toHaveBeenCalledWith("keyframe.paste", { item: "itm_a", payload, at: 140 });
    expect(keySelectionStore.getState().keys).toEqual([{ prop: "opacity", t: 40 }]);
  });

  it("reads only its own clipboard format", () => {
    expect(parseKeyPayload('{"version":1,"keys":[{"prop":"opacity","dt":0}]}')).not.toBeNull();
    expect(parseKeyPayload('{"version":1,"keys":[]}')).toBeNull();
    expect(parseKeyPayload('{"version":1,"items":[1],"tracks":[]}')).toBeNull();
    expect(parseKeyPayload("plain text")).toBeNull();
  });
});

describe("no key work while every lane is closed (a clip drag's updates)", () => {
  /** A document that counts how often anything in it is read. */
  function counted(): { project: Project; reads: () => number } {
    let reads = 0;
    const wrap = <T extends object>(target: T): T =>
      new Proxy(target, {
        get(t, prop, receiver) {
          reads += 1;
          const value = Reflect.get(t, prop, receiver);
          return typeof value === "object" && value !== null ? wrap(value as object) : value;
        },
        ownKeys(t) {
          reads += 1;
          return Reflect.ownKeys(t);
        },
      }) as T;
    return { project: wrap(PROJECT), reads: () => reads };
  }

  it("the lanes projection does not read the document when no lane is open", () => {
    const doc = counted();
    const selection = { item: "itm_a", keys: [{ prop: "opacity", t: 10 }], offset: 0 };
    for (let update = 0; update < 100; update += 1) {
      expect(laneGroups(doc.project, SEQ, [], selection, label)).toBe(NO_GROUPS);
    }
    expect(doc.reads()).toBe(0);
  });

  it("the toggle reads the clips once per document, selection and open lanes", () => {
    const toggle = createLanesToggle();
    const doc = counted();
    const selected = ["itm_a", "itm_b"];
    const open: string[] = [];
    expect(toggle(doc.project, selected, open)).toBe("closed");
    const first = doc.reads();
    expect(first).toBeGreaterThan(0);
    // The playhead moves, the view scrolls, something else renders: the same three objects.
    for (let n = 0; n < 100; n += 1) expect(toggle(doc.project, selected, open)).toBe("closed");
    expect(doc.reads()).toBe(first);
    expect(toggle(doc.project, selected, ["itm_a"])).toBe("open");
    expect(toggle(doc.project, ["itm_b"], ["itm_a"])).toBe("none");
    expect(toggle(null, selected, open)).toBe("none");
  });
});

describe("what is laid over the canvas follows the lanes' shape only", () => {
  it("the shape is empty with no lane open and the same while only keys move or get selected", () => {
    expect(lanesShapeOf(NO_GROUPS)).toBe("");
    const plain = laneGroups(PROJECT, SEQ, ["itm_a"], EMPTY_KEY_SELECTION, label);
    const selected = laneGroups(
      PROJECT,
      SEQ,
      ["itm_a"],
      { item: "itm_a", keys: [{ prop: "opacity", t: 10 }], offset: 0 },
      label,
    );
    expect(selected).not.toBe(plain);
    expect(lanesShapeOf(selected)).toBe(lanesShapeOf(plain));
    expect(lanesShapeOf(plain)).toBe("itm_a:2");
  });

  it("a new layout is announced once per call", () => {
    const before = lanesLaidStore.getState();
    lanesRelaid();
    expect(lanesLaidStore.getState()).toBe(before + 1);
  });
});
