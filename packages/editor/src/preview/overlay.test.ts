// SPDX-License-Identifier: GPL-3.0-or-later
// The overlay's geometry (WP-1M.04): the selected clip's outline in fractions of the canvas,
// placed exactly as cutweave-render's `LayerGeometry` places the picture.
import type { Project } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { SAFE_AREAS, selectionOutline } from "./overlay.ts";

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
) {
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
            items: {
              itm_1: {
                kind: "clip",
                id: "itm_1",
                media: "med_a",
                start: 0,
                duration: 1,
                visual: { transform: transform(clipTransform), opacity: 1 },
              },
            },
          },
        },
      },
    },
    custom: {},
  } as unknown as Project;
}

const sel = [{ kind: "item" as const, id: "itm_1" as never }];

describe("selectionOutline", () => {
  it("a full-frame clip of the sequence's aspect covers the canvas", () => {
    const o = selectionOutline(project({}, { width: 1280, height: 720 }), sel);
    expect(o).toEqual({
      left: 0,
      top: 0,
      width: 1,
      height: 1,
      originX: 0.5,
      originY: 0.5,
      rotation: 0,
    });
  });

  it("aspect-fits a portrait picture (pillarbox) before scale and position", () => {
    const o = selectionOutline(project({}, { width: 1080, height: 1920 }), sel);
    // Fit = 1080 / 1920: the picture is 607.5 px wide, centred.
    expect(o?.width).toBeCloseTo(607.5 / 1920);
    expect(o?.height).toBeCloseTo(1);
    expect(o?.left).toBeCloseTo((1920 - 607.5) / 2 / 1920);
    expect(o?.top).toBeCloseTo(0);
  });

  it("applies scale about the anchor, position from the centre, and rotation", () => {
    const o = selectionOutline(
      project(
        {
          scale: { x: 0.5, y: 0.5 },
          anchor: { x: 0, y: 0 },
          position: { x: -0.5, y: -0.5 },
          rotation: 30,
        },
        { width: 1920, height: 1080 },
      ),
      sel,
    );
    // Half size, anchored top-left at the canvas top-left (as cutweave-render's tests).
    expect(o?.left).toBeCloseTo(0);
    expect(o?.top).toBeCloseTo(0);
    expect(o?.width).toBeCloseTo(0.5);
    expect(o?.height).toBeCloseTo(0.5);
    expect(o?.originX).toBe(0);
    expect(o?.originY).toBe(0);
    expect(o?.rotation).toBe(30);
  });

  it("turns the picture by the media's display rotation before fitting", () => {
    const o = selectionOutline(project({}, { width: 1920, height: 1080, rotation: 90 }), sel);
    expect(o?.width).toBeCloseTo(607.5 / 1920);
  });

  it("is null without a single visual clip selected", () => {
    expect(selectionOutline(project({}, { width: 1, height: 1 }), [])).toBeNull();
    expect(selectionOutline(project({}, null), sel)).toBeNull();
    expect(selectionOutline(null, sel)).toBeNull();
    expect(
      selectionOutline(project({}, { width: 1, height: 1 }), [
        { kind: "item", id: "nope" as never },
      ]),
    ).toBeNull();
  });

  it("falls back to the canvas size when the probe has no picture size (stub media)", () => {
    const p = project({}, { width: 0, height: 0 });
    expect(selectionOutline(p, sel)).toMatchObject({ left: 0, top: 0, width: 1, height: 1 });
  });
});

describe("SAFE_AREAS", () => {
  it("are the SMPTE ST 2046-1 action- and title-safe insets", () => {
    expect(SAFE_AREAS.map((a) => a.inset)).toEqual([0.035, 0.05]);
  });
});
