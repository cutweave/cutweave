// SPDX-License-Identifier: GPL-3.0-or-later
// The overlay's small helpers (WP-1M.04, WP-2H.06).
import type { MediaAsset } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { displaySize, SAFE_AREAS } from "./overlay.ts";

const media = (width: number, height: number, rotation = 0) =>
  ({ probe: { streams: [{ kind: "video", width, height }], rotation } }) as unknown as MediaAsset;

describe("SAFE_AREAS", () => {
  it("are the SMPTE ST 2046-1 action- and title-safe insets", () => {
    expect(SAFE_AREAS.map((a) => a.inset)).toEqual([0.035, 0.05]);
  });
});

describe("displaySize", () => {
  it("turns the coded size by the display rotation", () => {
    expect(displaySize(media(1920, 1080))).toEqual([1920, 1080]);
    expect(displaySize(media(1920, 1080, 90))).toEqual([1080, 1920]);
    expect(displaySize(media(1920, 1080, 180))).toEqual([1920, 1080]);
    expect(displaySize(media(1920, 1080, 270))).toEqual([1080, 1920]);
  });
  it("is null without a picture size (audio, the dev stub)", () => {
    expect(displaySize(undefined)).toBeNull();
    expect(displaySize(media(0, 0))).toBeNull();
  });
});
