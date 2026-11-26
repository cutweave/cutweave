// SPDX-License-Identifier: GPL-3.0-or-later
// What the overlay finds to draw handles for (WP-2H.06), and the safe-area data pack.
import { readFileSync } from "node:fs";
import type { Project } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { ASPECT_GUIDES, aspectFrame, platformSafeArea, SAFE_AREA_PACK } from "./guides.ts";
import { buildScene, unionBounds } from "./scene.ts";

const FLICKS = 705_600_000;
const transform = (over: object = {}) => ({
  position: { x: 0, y: 0 },
  scale: { x: 1, y: 1 },
  rotation: 0,
  anchor: { x: 0.5, y: 0.5 },
  ...over,
});

function project(
  clipTransform: object,
  media: { width: number; height: number; rotation?: number } | null,
  extra: { hidden?: boolean; locked?: boolean } = {},
) {
  const item = (id: string, start: number) => ({
    kind: "clip",
    id,
    media: "med_a",
    start,
    duration: FLICKS,
    enabled: true,
    visual: { transform: transform(clipTransform), opacity: 1 },
  });
  return {
    meta: { name: "P" },
    settings: {},
    media: {
      med_a: {
        id: "med_a",
        kind: media === null ? "audio" : "video",
        probe: {
          streams:
            media === null
              ? [{ kind: "audio", width: null, height: null }]
              : [{ kind: "video", width: media.width, height: media.height }],
          rotation: media?.rotation ?? 0,
        },
      },
    },
    sequences: {
      seq_1: {
        id: "seq_1",
        format: { width: 1920, height: 1080, frame_rate: { num: 30, den: 1 } },
        track_order: ["trk_v"],
        tracks: {
          trk_v: {
            id: "trk_v",
            kind: "video",
            hidden: extra.hidden ?? false,
            locked: extra.locked ?? false,
            items: { itm_1: item("itm_1", 0), itm_2: item("itm_2", 2 * FLICKS) },
          },
        },
      },
    },
    custom: {},
  } as unknown as Project;
}

const sel = (...ids: string[]) => ids.map((id) => ({ kind: "item" as const, id: id as never }));

describe("buildScene", () => {
  it("finds the selected clip under the playhead, placed as the renderer places it", () => {
    const scene = buildScene(project({}, { width: 1280, height: 720 }), sel("itm_1"), 0);
    expect(scene?.selected).toHaveLength(1);
    expect(scene?.selected[0]?.geom.bounds).toEqual([0, 0, 1920, 1080]);
  });

  it("aspect-fits a portrait picture and turns it by the display rotation", () => {
    const portrait = buildScene(project({}, { width: 1080, height: 1920 }), sel("itm_1"), 0);
    expect(portrait?.selected[0]?.geom.bounds[2]).toBeCloseTo((1920 + 607.5) / 2);
    const rotated = buildScene(
      project({}, { width: 1920, height: 1080, rotation: 90 }),
      sel("itm_1"),
      0,
    );
    expect(rotated?.selected[0]?.geom.bounds[2]).toBeCloseTo((1920 + 607.5) / 2);
  });

  it("applies scale about the anchor, position and rotation", () => {
    const scene = buildScene(
      project(
        { scale: { x: 0.5, y: 0.5 }, anchor: { x: 0, y: 0 }, position: { x: -0.5, y: -0.5 } },
        { width: 1920, height: 1080 },
      ),
      sel("itm_1"),
      0,
    );
    expect(scene?.selected[0]?.geom.bounds).toEqual([0, 0, 960, 540]);
  });

  it("takes stub media (no picture size) as the canvas size", () => {
    const scene = buildScene(project({}, { width: 0, height: 0 }), sel("itm_1"), 0);
    expect(scene?.selected[0]?.geom.bounds).toEqual([0, 0, 1920, 1080]);
  });

  it("has no handles for audio, hidden tracks, a clip not under the playhead or nothing selected", () => {
    expect(buildScene(project({}, null), sel("itm_1"), 0)?.selected).toHaveLength(0);
    expect(
      buildScene(project({}, { width: 1, height: 1 }, { hidden: true }), sel("itm_1"), 0)?.selected,
    ).toHaveLength(0);
    expect(
      buildScene(project({}, { width: 1, height: 1 }), sel("itm_2"), 0)?.selected,
    ).toHaveLength(0);
    expect(buildScene(project({}, { width: 1, height: 1 }), [], 0)?.selected).toHaveLength(0);
    expect(buildScene(null, sel("itm_1"), 0)).toBeNull();
  });

  it("offers the other pictures on screen as snap targets and marks locked tracks", () => {
    const p = project(
      { scale: { x: 0.5, y: 0.5 } },
      { width: 1920, height: 1080 },
      { locked: true },
    );
    const both = buildScene(p, sel("itm_1"), 2 * FLICKS + 1);
    expect(both?.selected).toHaveLength(0);
    expect(both?.others).toHaveLength(1);
    expect(buildScene(p, sel("itm_1"), 0)?.selected[0]?.locked).toBe(true);
  });

  it("unions several clips' bounds", () => {
    const scene = buildScene(
      project({ scale: { x: 0.5, y: 0.5 } }, { width: 1920, height: 1080 }),
      sel("itm_1"),
      0,
    );
    expect(unionBounds(scene?.selected ?? [])).toEqual([480, 270, 1440, 810]);
    expect(unionBounds([])).toBeNull();
  });
});

describe("safe-area data pack", () => {
  it("has every platform id the export presets name, each with sane insets", () => {
    const manifest = JSON.parse(
      readFileSync(
        new URL("../../../../../plugins/presets-core/manifest.json", import.meta.url),
        "utf8",
      ),
    ) as { safe_area_ids: string[] };
    for (const id of manifest.safe_area_ids) {
      const zone = platformSafeArea(id);
      expect(zone, id).toBeDefined();
      const i = zone?.insets ?? { top: 1, bottom: 1, left: 1, right: 1 };
      expect(i.top + i.bottom).toBeLessThan(0.6);
      expect(i.left + i.right).toBeLessThan(0.5);
    }
    expect(SAFE_AREA_PACK.platforms.map((p) => p.id).sort()).toEqual(
      [...manifest.safe_area_ids].sort(),
    );
  });
});

describe("aspect frame guide", () => {
  it("is the largest centred box of the ratio inside the canvas", () => {
    const sixteenNine = 16 / 9;
    const f = aspectFrame(sixteenNine, 1);
    expect(f.left).toBeCloseTo((1 - 1080 / 1920) / 2);
    expect(f.top).toBe(0);
    const wide = aspectFrame(1, sixteenNine);
    expect(wide.top).toBeCloseTo((1 - 9 / 16) / 2);
    expect(ASPECT_GUIDES.map((a) => a.id)).toContain("9:16");
  });
});
