// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline panel's controller, for the things outside the panel that act on it: a track
// header's context menu starts its in-place rename (`beginRename`, F2) through it. One panel at a
// time; the panel sets it on mount and clears it on unmount.
import type { TimelineController } from "./controller.ts";

let active: TimelineController | null = null;

export function setActiveTimelineController(controller: TimelineController | null): void {
  active = controller;
}

/** The mounted timeline panel's controller, or `null` while the panel is closed. */
export function activeTimelineController(): TimelineController | null {
  return active;
}
