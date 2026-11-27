// SPDX-License-Identifier: GPL-3.0-or-later
// Linked groups in a selection (WP-2A.10): one clip per complete group, the clicked one first;
// a partly selected group keeps its members; partners join a clicked clip.
import type { Project } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { onePerGroup, withPartners } from "./groups.ts";

const item = (id: string, group: string | null) => ({ id, linked_group: group });
const project = {
  sequences: {
    s: {
      tracks: {
        v: { items: { v1: item("v1", "g1"), v2: item("v2", "g2"), t: item("t", null) } },
        a: { items: { a1: item("a1", "g1"), a2: item("a2", "g2") } },
      },
    },
  },
} as unknown as Project;

describe("linked groups in a selection", () => {
  it("counts a complete group once, as the member selected first", () => {
    expect(onePerGroup(project, ["a1", "v1"])).toEqual(["a1"]);
    expect(onePerGroup(project, ["v1", "a1", "v2", "a2", "t"])).toEqual(["v1", "v2", "t"]);
  });

  it("keeps the members of a group only partly selected", () => {
    expect(onePerGroup(project, ["v1", "v2"])).toEqual(["v1", "v2"]);
  });

  it("adds a clicked clip's partners after it", () => {
    expect(withPartners(project, ["a2"])).toEqual(["a2", "v2"]);
    expect(withPartners(project, ["t"])).toEqual(["t"]);
  });
});
