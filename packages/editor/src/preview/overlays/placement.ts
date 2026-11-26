// SPDX-License-Identifier: GPL-3.0-or-later
// The preview's picture menu (WP-2H.06): Fit, Fill, Center and the flips as `item.set_props`
// arguments for one clip at the playhead, worked out from the same geometry as the handles.
import type { ItemId, ItemSetProps, Project } from "@cutweave/protocol";
import { setPropsStep } from "./overlayEditor.ts";
import { buildScene } from "./scene.ts";
import { type PlaceMode, placeOnCanvas } from "./transformOps.ts";

/** `item.set_props` arguments placing one clip on the canvas; `null` when it has no picture there. */
export function placeArgs(
  project: Project | null,
  id: string,
  playhead: number,
  mode: PlaceMode,
): ItemSetProps | null {
  const scene = buildScene(project, [{ kind: "item", id: id as ItemId }], playhead);
  const layer = scene?.selected[0];
  if (scene === null || layer === undefined) return null;
  const out = placeOnCanvas(layer.state, layer.geom, scene.canvas, mode);
  const step = setPropsStep(
    id as ItemId,
    { transform: { position: out.position, scale: out.scale } },
    playhead,
  );
  return step.name === "item.set_props" ? step.input : null;
}

/** `item.set_props` arguments mirroring the clips on one axis (all take the first one's opposite). */
export function flipArgs(
  project: Project | null,
  ids: readonly string[],
  playhead: number,
  axis: "horizontal" | "vertical",
): ItemSetProps | null {
  const scene = buildScene(
    project,
    ids.map((id) => ({ kind: "item" as const, id: id as ItemId })),
    playhead,
  );
  const first = scene?.selected[0];
  if (scene === null || first === undefined) return null;
  const on = axis === "horizontal" ? !first.state.flipH : !first.state.flipV;
  const step = setPropsStep(
    first.item.id as ItemId,
    { flip: axis === "horizontal" ? { horizontal: on } : { vertical: on } },
    playhead,
  );
  if (step.name !== "item.set_props") return null;
  return { ...step.input, item: null, items: scene.selected.map((l) => l.item.id as ItemId) };
}
