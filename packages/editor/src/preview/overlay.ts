// SPDX-License-Identifier: GPL-3.0-or-later
// Geometry of the preview's overlay layer (WP-1M.04, arch §10 step 4): DOM drawn over the canvas
// in the same page. In v1 it shows safe-area guides and a display-only outline of the selected
// clip (no transform handles); positions are fractions of the canvas, so the overlay follows any
// size without knowing device pixels.
import type { Item, MediaAsset, Project, SelectionRef } from "@cutweave/protocol";
import { selectActiveSequenceId } from "../state/selectors.ts";

/**
 * Safe areas as insets from each edge (fractions of width and height): SMPTE ST 2046-1, action
 * safe 93% and title safe 90%.
 */
export const SAFE_AREAS = [
  { id: "action", inset: 0.035 },
  { id: "title", inset: 0.05 },
] as const;

/**
 * The selected clip's picture rectangle before rotation, in fractions of the canvas, plus the
 * rotation (clockwise degrees) about `origin` (fractions of the rectangle: the anchor).
 */
export interface Outline {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
  readonly originX: number;
  readonly originY: number;
  readonly rotation: number;
}

interface Vec {
  readonly x: number;
  readonly y: number;
}

const finite = (v: unknown, fallback: number): number =>
  typeof v === "number" && Number.isFinite(v) ? v : fallback;
const vec = (v: unknown, fallback: Vec): Vec => {
  const o = (typeof v === "object" && v !== null ? v : {}) as Record<string, unknown>;
  return { x: finite(o.x, fallback.x), y: finite(o.y, fallback.y) };
};

function findItem(project: Project, id: string): Item | null {
  for (const sequence of Object.values(project.sequences)) {
    for (const track of Object.values(sequence.tracks)) {
      const item = (track.items as Record<string, Item | undefined>)[id];
      if (item !== undefined) return item;
    }
  }
  return null;
}

/** The picture as displayed (coded size turned by the display rotation), or null if unknown. */
function displaySize(media: MediaAsset | undefined): [number, number] | null {
  const video = media?.probe?.streams?.find((s) => s.kind === "video");
  const w = finite(video?.width, 0);
  const h = finite(video?.height, 0);
  if (w <= 0 || h <= 0) return null;
  const turns = (((finite(media?.probe?.rotation, 0) % 360) + 360) % 360) as number;
  return turns === 90 || turns === 270 ? [h, w] : [w, h];
}

/**
 * The outline of the one selected visual clip, placed as cutweave-render places it (geometry.rs:
 * aspect-fit into the sequence canvas, `scale` on top, `anchor` in picture units placed at
 * `position` from the canvas centre, `rotation` clockwise about the anchor). Media whose probe
 * has no picture size (the dev stub) is taken as the canvas size. `null` for no selection,
 * several, audio-only media or an unknown item.
 */
export function selectionOutline(
  project: Project | null,
  selection: readonly SelectionRef[],
): Outline | null {
  if (project === null) return null;
  const items = selection.filter((ref) => ref.kind === "item");
  if (items.length !== 1 || items[0] === undefined) return null;
  const item = findItem(project, items[0].id);
  if (item === null) return null;
  const media = project.media[item.media];
  if (media === undefined || media.kind === "audio") return null;
  const seqId = selectActiveSequenceId(project);
  const format = seqId === null ? undefined : project.sequences[seqId]?.format;
  const cw = finite(format?.width, 0);
  const ch = finite(format?.height, 0);
  if (cw <= 0 || ch <= 0) return null;
  const [dw, dh] = displaySize(media) ?? [cw, ch];
  const t = item.visual?.transform;
  const scale = vec(t?.scale, { x: 1, y: 1 });
  const anchor = vec(t?.anchor, { x: 0.5, y: 0.5 });
  const position = vec(t?.position, { x: 0, y: 0 });
  const fit = Math.min(cw / dw, ch / dh);
  const w = dw * fit * scale.x;
  const h = dh * fit * scale.y;
  // The anchor lands at the positioned canvas point; the rectangle hangs off it.
  const px = cw * (0.5 + position.x);
  const py = ch * (0.5 + position.y);
  return {
    left: (px - anchor.x * w) / cw,
    top: (py - anchor.y * h) / ch,
    width: w / cw,
    height: h / ch,
    originX: anchor.x,
    originY: anchor.y,
    rotation: finite(t?.rotation, 0),
  };
}
