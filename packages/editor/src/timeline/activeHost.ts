// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline host of the mounted Timeline panel, for commands that act on the timeline from
// outside it (the command palette's Split at playhead, the menus' Rename track and zoom rows).
// `null` while the panel is closed.
import type { TimelineHost } from "./host.ts";

let active: TimelineHost | null = null;

/** Called by `useTimelineHost`. Returns the function that clears it. */
export function setActiveTimelineHost(host: TimelineHost): () => void {
  active = host;
  return () => {
    if (active === host) active = null;
  };
}

export function activeTimelineHost(): TimelineHost | null {
  return active;
}
