// SPDX-License-Identifier: GPL-3.0-or-later
// What counts as a use of an asset (WP-2B.02): its clips, and a sequence showing it as its
// background image, as the engine's `used_media` counts them.
import type { Project } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { selectMediaUsage } from "./cards.ts";

function project(fill: object | undefined): Project {
  return {
    media: {},
    sequences: {
      seq_1: {
        format: fill === undefined ? {} : { background_fill: fill },
        tracks: {
          trk_v: { items: { itm_1: { media: "med_clip" }, itm_2: { media: "med_clip" } } },
        },
      },
    },
  } as unknown as Project;
}

describe("media usage", () => {
  it("counts clips", () => {
    expect([...selectMediaUsage(project(undefined))]).toEqual([["med_clip", 2]]);
  });

  it("counts a sequence's background image as one use", () => {
    const usage = selectMediaUsage(project({ kind: "image", media: "med_paper", fit: "cover" }));
    expect(usage.get("med_paper")).toBe(1);
    expect(usage.get("med_clip")).toBe(2);
    const both = selectMediaUsage(project({ kind: "image", media: "med_clip", fit: "cover" }));
    expect(both.get("med_clip")).toBe(3);
  });

  it("counts nothing for a blurred or transparent background", () => {
    for (const fill of [{ kind: "blur", radius: 0.05, dim: 0.25 }, { kind: "transparent" }]) {
      expect([...selectMediaUsage(project(fill))]).toEqual([["med_clip", 2]]);
    }
  });
});
