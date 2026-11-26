// SPDX-License-Identifier: GPL-3.0-or-later
// What the inspector inspects when the timeline selects a whole linked group (WP-2A.10, timeline
// spec INV-341): the clip clicked stands for its group; a group only partly selected keeps each
// selected member; unlinked clips are each themselves.
import type { Project, SelectionRef } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { selectSelectedClips } from "./clips.ts";

const clip = (id: string, group: string | null) => ({
  id,
  start: 0,
  duration: 10,
  linked_group: group,
  enabled: true,
});

const project = {
  sequences: {
    seq: {
      id: "seq",
      format: { frame_rate: { num: 30, den: 1 } },
      tracks: {
        v: { id: "v", locked: false, items: { v1: clip("v1", "g1"), v2: clip("v2", "g2") } },
        a: { id: "a", locked: false, items: { a1: clip("a1", "g1"), a2: clip("a2", "g2") } },
        t: { id: "t", locked: false, items: { t1: clip("t1", null) } },
      },
    },
  },
} as unknown as Project;

const items = (...ids: string[]): SelectionRef[] =>
  ids.map((id) => ({ kind: "item", id }) as SelectionRef);
const ids = (selection: SelectionRef[]) =>
  selectSelectedClips(project, selection).map((c) => c.item.id);

describe("the inspector's clips", () => {
  it("shows a whole linked group as the member clicked first", () => {
    expect(ids(items("v1", "a1"))).toEqual(["v1"]);
    expect(ids(items("a1", "v1"))).toEqual(["a1"]);
  });

  it("keeps one clip per group across several groups, and unlinked clips as they are", () => {
    expect(ids(items("v1", "a1", "a2", "v2", "t1"))).toEqual(["v1", "a2", "t1"]);
  });

  it("keeps every selected member of a group only partly selected", () => {
    expect(ids(items("v1"))).toEqual(["v1"]);
    expect(ids(items("v1", "a2"))).toEqual(["v1", "a2"]);
  });
});
