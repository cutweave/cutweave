// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline controller of the mounted Timeline panel, for commands that act on the timeline
// from outside it (the command palette's Split at playhead). `null` while the panel is closed.
import type { TimelineController } from "./controller.ts";

let active: TimelineController | null = null;

/** Called by `useTimelineIntents`. Returns the function that clears it. */
export function setActiveTimelineController(controller: TimelineController): () => void {
  active = controller;
  return () => {
    if (active === controller) active = null;
  };
}

export function activeTimelineController(): TimelineController | null {
  return active;
}
