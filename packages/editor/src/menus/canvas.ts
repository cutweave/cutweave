// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline canvas as a menu source (WP-2H.12). The canvas is one element, so what is under the
// pointer is the hit-test's answer: its zone names the surface (`timeline.clip`, `timeline.lane`,
// ...), and its IDs and time are the target. The tables for those surfaces are WP-2H.13's; until a
// surface has one, a right-click there shows nothing.
import type { HitResult, HitZone, TimelinePointerEvent } from "@cutweave/timeline";
import type { OpenRequest } from "./host.tsx";
import type { OpenVia, ScreenPoint } from "./types.ts";

/** The surface of each hit zone (ADR-0026), or `null` where a right-click means nothing. */
export const ZONE_SURFACE: Readonly<Record<HitZone, string | null>> = {
  body: "timeline.clip",
  "left-edge": "timeline.edge",
  "right-edge": "timeline.edge",
  keyframe: "timeline.keyframe",
  lane: "timeline.lane",
  header: "timeline.track",
  "header-name": "timeline.track",
  "header-toggle": "timeline.track",
  ruler: "timeline.ruler",
  playhead: "timeline.playhead",
  none: null,
};

/** What a hit means as a menu target: the item or track hit, and where on the timeline. */
export function requestForHit(
  hit: HitResult,
  point: ScreenPoint,
  via: OpenVia,
  element: HTMLElement | null,
): OpenRequest | null {
  const surface = ZONE_SURFACE[hit.zone];
  if (surface === null) return null;
  const onTrack = surface === "timeline.track";
  const ids =
    onTrack && hit.trackId !== null
      ? [hit.trackId]
      : !onTrack && hit.itemId !== null
        ? [hit.itemId]
        : [];
  return {
    surface,
    point,
    via,
    element,
    target: {
      ids,
      place: { time: hit.time, ...(hit.trackId !== null && { trackId: hit.trackId }) },
      data: { zone: hit.zone, keyframeIndex: hit.keyframeIndex, keyframeTime: hit.keyframeTime },
    },
  };
}

/** The request for a `contextmenu` or `longpress` event of the canvas, or `null` for no menu. */
export function requestForCanvasEvent(e: TimelinePointerEvent): OpenRequest | null {
  if (e.type !== "contextmenu" && e.type !== "longpress") return null;
  const original = e.original;
  const element = original.target instanceof HTMLElement ? original.target : null;
  return requestForHit(
    e.hit,
    { x: original.clientX, y: original.clientY },
    e.type === "longpress" ? "touch" : "pointer",
    element,
  );
}
