// SPDX-License-Identifier: GPL-3.0-or-later
// Geometry and level-of-detail rules shared by the renderer and hit-testing, so they cannot drift.
import type { Rect } from "./model.ts";
import type { Theme } from "./theme.ts";
import type { Viewport } from "./viewport.ts";

/** Clips narrower than this (CSS px) draw no label. */
export const LABEL_MIN_PX = 40;
/** Clips narrower than this draw no thumbnail tiles or waveform. */
export const BODY_MEDIA_MIN_PX = 48;
/** Clips narrower than this draw and hit-test no keyframes. */
export const KEYFRAME_MIN_PX = 100;
/** Trim handle zone width at each clip edge (CSS px), shrunk to a third of narrow clips. */
export const HANDLE_PX = 8;
/** Clips narrower than this are still hittable over this many pixels. */
export const MIN_HIT_PX = 4;
/** At most this many labels per frame (fillText is the costliest primitive). */
export const MAX_LABELS = 400;
/** Filmstrip tile aspect (width / height). */
export const TILE_ASPECT = 16 / 9;

export type HeaderToggle = "lock" | "mute" | "hide";
export const HEADER_TOGGLES: readonly HeaderToggle[] = ["lock", "mute", "hide"];

/** Rect of header toggle `i` for a lane whose top edge is at canvas y `laneTop`. */
export function toggleRect(
  vp: Viewport,
  theme: Pick<Theme, "controlSm" | "space1" | "space2">,
  laneTop: number,
  laneHeight: number,
  i: number,
  out: MutableRect = { x: 0, y: 0, w: 0, h: 0 },
): Rect {
  const size = theme.controlSm;
  out.x = toggleX(vp, theme, i);
  out.y = laneTop + (laneHeight - size) / 2;
  out.w = size;
  out.h = size;
  return out;
}

/** A rectangle the draw loop fills in place (no allocation per frame). */
export interface MutableRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

function toggleX(
  vp: Viewport,
  theme: Pick<Theme, "controlSm" | "space1" | "space2">,
  i: number,
): number {
  const n = HEADER_TOGGLES.length;
  return vp.headerWidth - theme.space2 - (n - i) * theme.controlSm - (n - 1 - i) * theme.space1;
}

/**
 * Rect of the track name in a header: right of the kind icon, left of the first toggle, the full
 * lane tall. The renderer draws the name here and hit-testing opens the rename input from it.
 */
export function nameRect(
  vp: Viewport,
  theme: Pick<Theme, "controlSm" | "iconSm" | "space1" | "space2">,
  laneTop: number,
  laneHeight: number,
  out: MutableRect = { x: 0, y: 0, w: 0, h: 0 },
): Rect {
  const x = theme.space2 + theme.iconSm + theme.space2;
  out.x = x;
  out.y = laneTop;
  out.w = Math.max(0, toggleX(vp, theme, 0) - theme.space2 - x);
  out.h = laneHeight;
  return out;
}

/** Vertical center of the keyframe row of a clip whose top is `clipTop` and height `clipH`. */
export function keyframeRowY(
  theme: Pick<Theme, "space1" | "space1_5">,
  clipTop: number,
  clipH: number,
): number {
  return clipTop + clipH - theme.space1_5 - theme.space1;
}

/** Keyframes need a clip body at least this tall; `sm` tracks show none. */
export function showsKeyframes(clipH: number, theme: Pick<Theme, "trackSm">): boolean {
  return clipH > theme.trackSm;
}

/** Height of the label band under the header strip. */
export function labelBand(theme: Pick<Theme, "captionPx" | "space1">): number {
  return theme.captionPx + theme.space1 + 1;
}
