// SPDX-License-Identifier: GPL-3.0-or-later
import { applyPatch, type Json, type PatchOp, type Project } from "@cutweave/protocol";
import { EMPTY_MODEL } from "@cutweave/timeline";
import { describe, expect, it } from "vitest";
import {
  mediaName,
  selectActiveSequenceId,
  selectInOut,
  selectMedia,
  selectorWork,
  selectSequenceSummary,
  selectTimelineItems,
  selectTimelineModel,
} from "./selectors.ts";

const clip = (id: string, start: number, duration: number, extra: object = {}) => ({
  kind: "clip",
  id,
  name: null,
  start,
  duration,
  enabled: true,
  linked_group: null,
  attached_to: null,
  markers: {},
  media: "med_a",
  source_in: 10,
  audio: { gain: 0 },
  visual: { transform: {}, opacity: 1 },
  custom: {},
  ...extra,
});

const track = (id: string, kind: string, items: object[], extra: object = {}) => ({
  id,
  kind,
  name: id.toUpperCase(),
  items: Object.fromEntries(items.map((i) => [(i as { id: string }).id, i])),
  muted: false,
  hidden: false,
  locked: false,
  solo: false,
  magnetic: false,
  height: null,
  gain: 0,
  custom: {},
  ...extra,
});

const project = (): Project =>
  ({
    meta: { name: "P" },
    settings: {},
    media: {
      med_a: {
        id: "med_a",
        kind: "video",
        path_abs: "C:\\media\\beach.mp4",
        path_rel: null,
        probe: { duration: 1000 },
      },
      med_b: { id: "med_b", kind: "audio", path_abs: null, path_rel: "audio/song.wav", probe: {} },
    },
    sequences: {
      seq_2: {
        id: "seq_2",
        name: "Later",
        tracks: {},
        track_order: [],
        markers: {},
        in_out: null,
        format: fmt(),
        custom: {},
      },
      seq_1: {
        id: "seq_1",
        name: "Main",
        format: fmt(),
        track_order: ["v2", "v1", "a1"],
        markers: {},
        in_out: null,
        tracks: {
          v1: track("v1", "video", [clip("itm_2", 500, 100), clip("itm_1", 0, 400)], {
            magnetic: true,
          }),
          v2: track("v2", "video", [clip("itm_3", 50, 10, { name: "Title", enabled: false })]),
          a1: track("a1", "audio", [clip("itm_4", 0, 400, { media: "med_b" })], { muted: true }),
        },
        custom: {},
      },
    },
    custom: {},
  }) as unknown as Project;

function fmt() {
  return { width: 1920, height: 1080, frame_rate: { num: 30000, den: 1001 } };
}

describe("markers, in/out and track heights (WP-2A.09)", () => {
  /** The test project with markers, an in/out and a tall track on its main sequence. */
  function marked(): Project {
    const p = project();
    const seq = (p.sequences as unknown as Record<string, Record<string, unknown>>).seq_1 as Record<
      string,
      unknown
    >;
    seq.markers = {
      mrk_b: { id: "mrk_b", t: 300, duration: 50, label: "Range", color: "#f2b134", kind: "user" },
      mrk_a: { id: "mrk_a", t: 100, duration: null, label: "Beat", color: "#ffffff", kind: "beat" },
    };
    seq.in_out = { start: 20, end: 200 };
    const tracks = seq.tracks as Record<string, Record<string, unknown>>;
    (tracks.v1 as Record<string, unknown>).height = 96;
    return p;
  }

  it("projects sequence markers by time, with their color and kind", () => {
    expect(selectTimelineModel(marked()).markers).toEqual([
      { id: "mrk_a", t: 100, label: "Beat", color: "#ffffff", kind: "beat" },
      { id: "mrk_b", t: 300, duration: 50, label: "Range", color: "#f2b134", kind: "user" },
    ]);
    expect(selectTimelineModel(project()).markers).toBeUndefined();
  });

  it("passes the height the user set, and none for the default", () => {
    const tracks = selectTimelineModel(marked()).tracks;
    expect(tracks.find((t) => t.id === "v1")?.height).toBe(96);
    expect(tracks.find((t) => t.id === "a1")?.height).toBeUndefined();
  });

  it("shades the in/out range and keeps its identity while it is unchanged", () => {
    const p = marked();
    const range = selectInOut(p);
    expect(range).toEqual({ in: 20, out: 200 });
    expect(selectInOut(p)).toBe(range);
    expect(selectInOut(project())).toBeNull();
    expect(selectInOut(null)).toBeNull();
  });
});

describe("selectSequenceSummary", () => {
  it("reads the active sequence's canvas and rate and ends at its last item", () => {
    expect(selectSequenceSummary(project())).toEqual({
      width: 1920,
      height: 1080,
      frameRate: { num: 30000, den: 1001 },
      duration: 600,
    });
  });

  it("is 0 long for an empty sequence and null without a project", () => {
    const p = project();
    const sequences = p.sequences as unknown as Record<string, unknown>;
    const empty = { ...p, sequences: { seq_2: sequences.seq_2 } } as unknown as Project;
    expect(selectSequenceSummary(empty)?.duration).toBe(0);
    expect(selectSequenceSummary(null)).toBeNull();
  });
});

describe("selectActiveSequenceId", () => {
  it("is the lowest sequence ID, or null without a project", () => {
    expect(selectActiveSequenceId(project())).toBe("seq_1");
    expect(selectActiveSequenceId(null)).toBeNull();
  });
});

describe("selectTimelineModel", () => {
  const model = selectTimelineModel(project());

  it("lists tracks top to bottom in track_order, with the renderer's kinds and flags", () => {
    expect(model.tracks.map((t) => [t.id, t.kind])).toEqual([
      ["v2", "media"],
      ["v1", "media"],
      ["a1", "audio"],
    ]);
    expect(model.tracks.find((t) => t.id === "v1")?.main).toBe(true);
    expect(model.tracks.find((t) => t.id === "a1")?.muted).toBe(true);
  });

  it("takes the frame rate from the sequence format", () => {
    expect(model.fps).toEqual({ num: 30000, den: 1001 });
  });

  it("maps every item by ID with flicks, track and a label", () => {
    const byId = Object.fromEntries(model.items.map((i) => [i.id, i]));
    expect(Object.keys(byId).sort()).toEqual(["itm_1", "itm_2", "itm_3", "itm_4"]);
    expect(byId.itm_1).toMatchObject({
      trackId: "v1",
      start: 0,
      duration: 400,
      label: "beach.mp4",
    });
    expect(byId.itm_3).toMatchObject({ label: "Title", state: { disabled: true } });
    expect(byId.itm_4?.label).toBe("song.wav");
  });

  it("orders items by start within a track, track by track", () => {
    expect(model.items.map((i) => i.id)).toEqual(["itm_3", "itm_1", "itm_2", "itm_4"]);
  });

  it("points thumbnails and waveforms at the media and source position", () => {
    const byId = Object.fromEntries(model.items.map((i) => [i.id, i]));
    expect(byId.itm_1?.thumbnails).toEqual({ mediaId: "med_a", sourceIn: 10 });
    expect(byId.itm_4?.waveform).toEqual({ mediaId: "med_b", sourceIn: 10 });
    expect(byId.itm_4?.thumbnails).toBeUndefined();
  });

  it("carries the linked group and the trim bounds of the media (none for stills)", () => {
    const p = project();
    const seq = p.sequences["seq_1" as never] as unknown as {
      tracks: Record<string, { items: Record<string, Record<string, unknown>> }>;
    };
    const v1 = seq.tracks.v1?.items;
    if (v1?.itm_1) v1.itm_1.linked_group = "grp_1";
    (p.media as unknown as Record<string, object>).med_c = {
      id: "med_c",
      kind: "image",
      path_abs: "/still.png",
      path_rel: null,
      probe: { duration: null },
    };
    if (v1?.itm_2) v1.itm_2.media = "med_c";
    const byId = Object.fromEntries(selectTimelineModel(p).items.map((i) => [i.id, i]));
    expect(byId.itm_1).toMatchObject({
      linkedGroup: "grp_1",
      source: { sourceIn: 10, mediaDuration: 1000 },
    });
    expect(byId.itm_2?.source).toEqual({ sourceIn: 10, mediaDuration: null, still: true });
    expect(byId.itm_4?.source).toEqual({ sourceIn: 10, mediaDuration: null });
    expect(byId.itm_4?.linkedGroup).toBeUndefined();
  });

  it("is memoized per project object and empty without one", () => {
    const p = project();
    expect(selectTimelineModel(p)).toBe(selectTimelineModel(p));
    expect(selectTimelineModel(null)).toBe(EMPTY_MODEL);
    expect(selectTimelineModel(p, "seq_x" as never)).toBe(EMPTY_MODEL);
    expect(selectTimelineItems(p)).toBe(selectTimelineModel(p).items);
  });
});

describe("text items (WP-3A.01)", () => {
  const text = (id: string, words: string, extra: object = {}) => ({
    kind: "text",
    id,
    name: null,
    start: 10,
    duration: 90,
    enabled: true,
    linked_group: null,
    attached_to: null,
    markers: {},
    content: { runs: [{ text: words }] },
    style: { source: "named", name: "title" },
    box: { width: null, height: null },
    animation: null,
    visual: { transform: {}, opacity: 1 },
    custom: {},
    ...extra,
  });

  it("shows a text item as a plain clip on the graphics lane, labelled by its words", () => {
    const p = project();
    const seq = (p.sequences as unknown as Record<string, unknown>).seq_1 as {
      tracks: Record<string, unknown>;
      track_order: string[];
    };
    seq.tracks.t1 = track("t1", "text", [
      text("itm_9", "Launch day\nsecond line"),
      text("itm_10", "x", { name: "Opening" }),
    ]);
    seq.track_order.unshift("t1");
    const model = selectTimelineModel(p);
    const byId = Object.fromEntries(model.items.map((i) => [i.id, i]));
    expect(byId.itm_9).toMatchObject({
      trackId: "t1",
      kind: "graphics",
      start: 10,
      duration: 90,
      label: "Launch day",
    });
    expect(byId.itm_10?.label).toBe("Opening");
    expect(model.tracks[0]).toMatchObject({ id: "t1", kind: "graphics" });
  });
});

describe("selectMedia", () => {
  it("lists the media by ID with names and durations", () => {
    expect(selectMedia(project())).toEqual([
      { id: "med_a", kind: "video", name: "beach.mp4", duration: 1000 },
      { id: "med_b", kind: "audio", name: "song.wav", duration: null },
    ]);
    expect(selectMedia(null)).toEqual([]);
  });

  it("names a pathless asset by its ID", () => {
    expect(mediaName(undefined, "med_z" as never)).toBe("med_z");
  });
});

// Issues #230 and #231: every `doc.patched` gives the editor a new project object, and the
// timeline's model was built again from all of it: 5,000 items derived and sorted for a drag
// update that moved one clip. Counted, not timed, so the tests hold on a busy machine.
describe("selectTimelineModel after a patch", () => {
  /** A project with `n` clips on `v1` and a few on the other tracks. */
  function large(n: number): Project {
    const p = project();
    const seq = (p.sequences as unknown as Record<string, { tracks: Record<string, unknown> }>)
      .seq_1;
    if (seq === undefined) throw new Error("no sequence");
    const clips = Array.from({ length: n }, (_, i) => clip(`itm_m${i}`, i * 100, 100));
    seq.tracks.v1 = track("v1", "video", clips, { magnetic: true });
    return p;
  }
  const patched = (p: Project, ops: object[]): Project =>
    applyPatch(p as unknown as Json, ops as unknown as PatchOp[]) as unknown as Project;
  const start = (item: string, old: number, value: number) => ({
    op: "replace",
    path: `/sequences/seq_1/tracks/v1/items/${item}/start`,
    old,
    value,
  });

  it("derives only the items that changed, and gives the model a fresh build would", () => {
    const counts = [10, 1000].map((n) => {
      const before = large(n);
      const first = selectTimelineModel(before);
      const after = patched(before, [start("itm_m3", 300, 310)]);
      const work = selectorWork.items;
      const model = selectTimelineModel(after);
      const derived = selectorWork.items - work;
      // The same model as one built from a copy that shares nothing with what was seen before.
      expect(model).toEqual(selectTimelineModel(structuredClone(after)));
      expect(model).not.toBe(first);
      // Every other item is the object it was: what is drawn from it stays as it is.
      const moved = model.items.find((i) => i.id === "itm_m3");
      expect(moved?.start).toBe(310);
      expect(moved).not.toBe(first.items.find((i) => i.id === "itm_m3"));
      const same = model.items.filter((item, i) => item === first.items[i]).length;
      expect(same).toBe(model.items.length - 1);
      // The untouched tracks keep their lanes too.
      expect(model.tracks[0]).toBe(first.tracks[0]);
      return derived;
    });
    expect(counts).toEqual([1, 1]);
  });

  it("derives a ripple's items once, in their new order", () => {
    const before = large(50);
    selectTimelineModel(before);
    // The last clip moves to the front and the others make room.
    const ops = [
      ...Array.from({ length: 49 }, (_, i) => start(`itm_m${i}`, i * 100, i * 100 + 100)),
      start("itm_m49", 4900, 0),
    ];
    const after = patched(before, ops);
    const work = selectorWork.items;
    const model = selectTimelineModel(after);
    expect(selectorWork.items - work).toBe(50);
    const lane = model.items.filter((i) => i.trackId === "v1").map((i) => i.id);
    expect(lane[0]).toBe("itm_m49");
    expect(lane[1]).toBe("itm_m0");
    expect(model).toEqual(selectTimelineModel(structuredClone(after)));
  });

  it("derives an item again when its media or its track's kind changes", () => {
    const before = large(5);
    const first = selectTimelineModel(before);
    const renamed = patched(before, [
      {
        op: "replace",
        path: "/media/med_a/path_abs",
        old: "C:\\media\\beach.mp4",
        value: "C:\\media\\dune.mp4",
      },
    ]);
    const model = selectTimelineModel(renamed);
    expect(model.items.find((i) => i.id === "itm_m0")?.label).toBe("dune.mp4");
    expect(model).toEqual(selectTimelineModel(structuredClone(renamed)));
    // The audio clip plays other media: it is the item it was.
    const audio = (m: typeof model) => m.items.find((i) => i.id === "itm_4");
    expect(audio(model)).toBe(audio(first));
    const muted = patched(renamed, [
      { op: "replace", path: "/sequences/seq_1/tracks/v1/muted", old: false, value: true },
    ]);
    const lanes = selectTimelineModel(muted);
    expect(lanes.tracks.find((t) => t.id === "v1")?.muted).toBe(true);
    expect(lanes.items.find((i) => i.id === "itm_m0")).toBe(
      model.items.find((i) => i.id === "itm_m0"),
    );
    expect(lanes).toEqual(selectTimelineModel(structuredClone(muted)));
  });

  it("reads the length of the sequence from the tracks that changed", () => {
    const before = large(20);
    expect(selectSequenceSummary(before)?.duration).toBe(2000);
    const after = patched(before, [start("itm_m19", 1900, 2400)]);
    expect(selectSequenceSummary(after)?.duration).toBe(2500);
  });
});
