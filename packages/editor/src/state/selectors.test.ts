// SPDX-License-Identifier: GPL-3.0-or-later
import type { Project } from "@cutweave/protocol";
import { EMPTY_MODEL } from "@cutweave/timeline";
import { describe, expect, it } from "vitest";
import {
  mediaName,
  selectActiveSequenceId,
  selectInOut,
  selectMedia,
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
