// SPDX-License-Identifier: GPL-3.0-or-later
// The Source view's marks, the arguments of its Insert and Overwrite, and match frame (WP-2A.14).
import type { MediaId, Project, TrackId } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { createPlaybackStore } from "../state/playbackStore.ts";
import { createViewStore } from "../state/viewStore.ts";
import { sampleProject, sec } from "../testing/fixtures.ts";
import { clearSourceMarks, markedRange, markSource, withMark } from "./marks.ts";
import { matchFrameAt } from "./matchFrame.ts";
import { planPlacement } from "./place.ts";

const MEDIA = "med_a" as MediaId;
const LENGTH = sec(10);

describe("withMark", () => {
  it("sets one mark and keeps the other while it is on the right side", () => {
    expect(withMark(undefined, "in", sec(2), LENGTH)).toEqual({ in: sec(2), out: null });
    expect(withMark({ in: sec(2), out: null }, "out", sec(5), LENGTH)).toEqual({
      in: sec(2),
      out: sec(5),
    });
    // An in at or after the out drops the out; an out at or before the in drops the in.
    expect(withMark({ in: sec(2), out: sec(5) }, "in", sec(5), LENGTH)).toEqual({
      in: sec(5),
      out: null,
    });
    expect(withMark({ in: sec(2), out: sec(5) }, "out", sec(2), LENGTH)).toEqual({
      in: null,
      out: sec(2),
    });
  });

  it("leaves the marks alone where a mark would place nothing", () => {
    const marks = { in: sec(2), out: sec(5) };
    expect(withMark(marks, "out", 0, LENGTH)).toBe(marks);
    expect(withMark(marks, "in", LENGTH, LENGTH)).toBe(marks);
    expect(withMark(undefined, "out", sec(99), LENGTH)).toEqual({ in: null, out: LENGTH });
  });

  it("markedRange fills the missing end from the media", () => {
    expect(markedRange(undefined, LENGTH)).toBeNull();
    expect(markedRange({ in: sec(2), out: null }, LENGTH)).toEqual({ in: sec(2), out: LENGTH });
    expect(markedRange({ in: null, out: sec(5) }, LENGTH)).toEqual({ in: 0, out: sec(5) });
  });
});

describe("markSource", () => {
  it("marks the source on screen at its own playhead and clears both", () => {
    const view = createViewStore();
    const playback = createPlaybackStore();
    const deps = { view, playback };
    expect(markSource(deps, "in")).toBe(false);
    playback.setState({
      source: { media: MEDIA, duration: LENGTH, fps: { num: 30, den: 1 } },
      sourcePlayhead: sec(2),
    });
    expect(markSource(deps, "in")).toBe(true);
    playback.setState({ sourcePlayhead: sec(5) });
    markSource(deps, "out");
    expect(view.getState().sourceMarks).toEqual({ [MEDIA]: { in: sec(2), out: sec(5) } });
    // The timeline's playhead and the document are not involved.
    expect(view.getState().playhead).toBe(0);
    clearSourceMarks(deps);
    expect(view.getState().sourceMarks).toEqual({});
  });
});

describe("planPlacement", () => {
  const source = { id: MEDIA, kind: "video" as const, duration: LENGTH };
  const base = {
    project: sampleProject(),
    playhead: sec(6),
    track: "trk_v" as TrackId,
    source,
    marks: { in: sec(2), out: sec(5) },
  };

  it("insert places the marked range at the playhead, splitting what is there", () => {
    expect(planPlacement({ ...base, mode: "insert" })).toEqual({
      ok: true,
      shortened: false,
      args: {
        sequence: "seq_1",
        track: "trk_v",
        media: MEDIA,
        at: sec(6),
        source_in: sec(2),
        duration: sec(3),
        mode: "insert",
      },
    });
  });

  it("overwrite sends no mode, and the picture alone sends link: false", () => {
    const plan = planPlacement({ ...base, mode: "overwrite", link: false });
    expect(plan.ok && plan.args).toEqual({
      sequence: "seq_1",
      track: "trk_v",
      media: MEDIA,
      at: sec(6),
      source_in: sec(2),
      duration: sec(3),
      link: false,
    });
  });

  it("the timeline's in point wins over the playhead and the source marks give the length", () => {
    const project = sampleProject();
    const sequence = Object.values(project.sequences)[0] as unknown as { in_out: object };
    sequence.in_out = { start: sec(1), end: sec(2) };
    const both = planPlacement({ ...base, project, mode: "insert" });
    expect(both.ok && [both.args.at, both.args.source_in, both.args.duration]).toEqual([
      sec(1),
      sec(2),
      sec(3),
    ]);
    const timelineOnly = planPlacement({ ...base, project, marks: undefined, mode: "insert" });
    expect(timelineOnly.ok && [timelineOnly.args.at, timelineOnly.args.duration]).toEqual([
      sec(1),
      sec(1),
    ]);
  });

  it("without marks the whole asset goes in; without a track nothing does", () => {
    const whole = planPlacement({ ...base, marks: undefined, mode: "insert" });
    expect(whole.ok && [whole.args.source_in, whole.args.duration]).toEqual([0, LENGTH]);
    expect(planPlacement({ ...base, track: null, mode: "insert" })).toEqual({
      ok: false,
      reason: "no_track",
    });
    expect(planPlacement({ ...base, marks: { in: LENGTH, out: null }, mode: "insert" })).toEqual({
      ok: false,
      reason: "invalid",
    });
  });
});

describe("matchFrameAt", () => {
  const withClip = (edits: object): Project => sampleProject({ itm_v: edits, itm_a: edits });

  it("finds the frame of the picture under the playhead and the clip's source range", () => {
    // The sample clip: start 1 s, 4 s long; here it plays its media from 2 s.
    const project = withClip({ source_in: sec(2) });
    expect(matchFrameAt(project, sec(3))).toEqual({
      item: "itm_v",
      media: MEDIA,
      at: sec(4),
      in: sec(2),
      out: sec(6),
    });
    // A selected clip under the playhead is preferred (here the sound).
    expect(matchFrameAt(project, sec(3), ["itm_a"])?.item).toBe("itm_a");
    // Nothing under the playhead: the clip's end is exclusive.
    expect(matchFrameAt(project, sec(5))).toBeNull();
    expect(matchFrameAt(project, 0)).toBeNull();
    expect(matchFrameAt(null, 0)).toBeNull();
  });

  it("follows the clip's speed and direction", () => {
    const fast = withClip({ source_in: sec(1), speed: { num: 2, den: 1 } });
    expect(matchFrameAt(fast, sec(2))).toMatchObject({ at: sec(3), in: sec(1), out: sec(9) });
    const reversed = withClip({ source_in: sec(1), reverse: true });
    expect(matchFrameAt(reversed, sec(2))).toMatchObject({ at: sec(4), in: sec(1), out: sec(5) });
  });
});
