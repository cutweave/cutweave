// SPDX-License-Identifier: GPL-3.0-or-later
// What the overlay draws handles for (WP-2H.06): the selected clips that show a picture at the
// playhead, each with its placement there, and the other pictures on screen (snap targets).
import type { ClipItem as Item, Project, SelectionRef } from "@cutweave/protocol";
import { selectActiveSequenceId } from "../../state/selectors.ts";
import {
  type LayerGeom,
  type LayerState,
  layerAt,
  layerGeometry,
  pictureSize,
  type Size,
} from "./geometry.ts";

export interface SceneLayer {
  readonly id: string;
  readonly item: Item;
  readonly state: LayerState;
  readonly display: Size;
  readonly geom: LayerGeom;
  /** The clip's track is locked: handles show but do nothing. */
  readonly locked: boolean;
}

export interface Scene {
  readonly canvas: Size;
  readonly playhead: number;
  readonly selected: readonly SceneLayer[];
  /** Bounds (canvas px) of the other pictures on screen, for snapping. */
  readonly others: readonly (readonly [number, number, number, number])[];
}

const onScreen = (item: Item, playhead: number): boolean =>
  item.enabled !== false && playhead >= item.start && playhead < item.start + item.duration;

/** The scene at `playhead`, or `null` without a project, a sequence or a canvas. */
export function buildScene(
  project: Project | null,
  selection: readonly SelectionRef[],
  playhead: number,
): Scene | null {
  if (project === null) return null;
  const seqId = selectActiveSequenceId(project);
  const sequence = seqId === null ? undefined : project.sequences[seqId];
  const w = sequence?.format.width ?? 0;
  const h = sequence?.format.height ?? 0;
  if (sequence === undefined || !(w > 0) || !(h > 0)) return null;
  const canvas: Size = { w, h };
  const picked = new Set(selection.filter((s) => s.kind === "item").map((s) => s.id as string));
  const selected: SceneLayer[] = [];
  const others: [number, number, number, number][] = [];
  for (const track of Object.values(sequence.tracks)) {
    if (track.kind !== "video" || track.hidden) continue;
    for (const item of Object.values(track.items) as Item[]) {
      if (!onScreen(item, playhead)) continue;
      const media = project.media[item.media];
      if (media === undefined || media.kind === "audio") continue;
      const display = pictureSize(media, canvas);
      const state = layerAt(item, playhead);
      const geom = layerGeometry(state, display, canvas);
      if (geom === null) continue;
      if (picked.has(item.id)) {
        selected.push({ id: item.id, item, state, display, geom, locked: track.locked });
      } else {
        others.push([...geom.bounds]);
      }
    }
  }
  // The selection's order, so the first selected clip is the one a multi-drag follows.
  const order = selection.filter((s) => s.kind === "item").map((s) => s.id as string);
  selected.sort((a, b) => order.indexOf(a.id) - order.indexOf(b.id));
  return { canvas, playhead, selected, others };
}

/** The axis-aligned bounds of several layers: `[x0, y0, x1, y1]`. */
export function unionBounds(
  layers: readonly SceneLayer[],
): readonly [number, number, number, number] | null {
  if (layers.length === 0) return null;
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const l of layers) {
    x0 = Math.min(x0, l.geom.bounds[0]);
    y0 = Math.min(y0, l.geom.bounds[1]);
    x1 = Math.max(x1, l.geom.bounds[2]);
    y1 = Math.max(y1, l.geom.bounds[3]);
  }
  return [x0, y0, x1, y1];
}
