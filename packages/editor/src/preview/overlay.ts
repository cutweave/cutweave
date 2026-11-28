// SPDX-License-Identifier: GPL-3.0-or-later
// Small helpers of the preview's overlay layer (WP-1M.04, WP-2H.06): the generic safe areas and
// where a clip and its picture size come from. The clip's box and handles are
// `overlays/TransformOverlay.tsx`, placed with `overlays/geometry.ts`.
import type { ClipItem as Item, MediaAsset, Project } from "@cutweave/protocol";
import { SAFE_AREA_PACK } from "./overlays/guides.ts";

/**
 * The generic safe areas as insets from each edge (fractions of width and height): SMPTE
 * ST 2046-1, action safe 93% and title safe 90%. They are the first part of the safe-area data
 * pack (`overlays/safe-areas.json`), which adds each platform's own zone.
 */
export const SAFE_AREAS = SAFE_AREA_PACK.generic;

const finite = (v: unknown, fallback: number): number =>
  typeof v === "number" && Number.isFinite(v) ? v : fallback;

export function findItem(project: Project, id: string): Item | null {
  for (const sequence of Object.values(project.sequences)) {
    for (const track of Object.values(sequence.tracks)) {
      const item = (track.items as Record<string, Item | undefined>)[id];
      if (item !== undefined) return item;
    }
  }
  return null;
}

/** The picture as displayed (coded size turned by the display rotation), or null if unknown. */
export function displaySize(media: MediaAsset | undefined): [number, number] | null {
  const video = media?.probe?.streams?.find((s) => s.kind === "video");
  const w = finite(video?.width, 0);
  const h = finite(video?.height, 0);
  if (w <= 0 || h <= 0) return null;
  const turns = (((finite(media?.probe?.rotation, 0) % 360) + 360) % 360) as number;
  return turns === 90 || turns === 270 ? [h, w] : [w, h];
}
