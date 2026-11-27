// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline canvas's keyboard path (WP-2H.12): the canvas is one focusable element, so
// Shift+F10 and the Menu key cannot ask a child what to open. They open the clip menu at the
// selected clip's lower-left corner, or the playhead's menu where the playhead is, and the
// pointer path (`canvas.ts`) stays with the canvas.
import type { TimelineView } from "@cutweave/timeline";
import { docStore } from "../state/docStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { onePerGroup } from "../timeline/groups.ts";
import { itemSelection } from "./adapters.ts";
import { registerContextSurface, type SurfaceResolver } from "./host.tsx";

/** What the keyboard opens on the timeline canvas, from the selection or the playhead. */
export function timelineKeyboardResolver(
  getView: () => TimelineView | null,
  canvas: () => HTMLElement | null,
): SurfaceResolver {
  return () => {
    const view = getView();
    const element = canvas();
    if (view === null || element === null) return null;
    const box = element.getBoundingClientRect();
    // One clip per linked group, as the clip menu counts them (WP-2A.10).
    const ids = onePerGroup(docStore.getState().project, itemSelection.current());
    const rect = ids[0] === undefined ? null : view.itemRect(ids[0]);
    if (rect !== null && ids[0] !== undefined) {
      return {
        surface: "timeline.clip",
        target: { ids },
        point: { x: Math.round(box.left + rect.x), y: Math.round(box.top + rect.y + rect.h) },
      };
    }
    const playhead = viewStore.getState().playhead;
    return {
      surface: "timeline.playhead",
      target: { ids: [], place: { time: playhead } },
      point: { x: Math.round(box.left + view.timeToX(playhead)), y: Math.round(box.top) },
    };
  };
}

/** Registers the canvas wrapper for the keyboard; the pointer is the canvas's own. */
export function registerTimelineCanvas(
  wrapper: HTMLElement,
  getView: () => TimelineView | null,
): () => void {
  return registerContextSurface(
    wrapper,
    timelineKeyboardResolver(getView, () => wrapper.querySelector("canvas")),
    { pointerOwned: true, name: "timeline.canvas" },
  );
}
