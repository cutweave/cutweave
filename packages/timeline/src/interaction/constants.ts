// SPDX-License-Identifier: GPL-3.0-or-later
// Tunables of the interaction machine (WP-2A.08).
import { FLICKS_PER_SECOND } from "../time.ts";

/** Pointer travel (CSS px) that turns a press into a drag. */
export const DRAG_THRESHOLD_PX = 3;

/** Default snap distance in CSS px (the Timeline preference "Snap distance", spec §6). */
export const DEFAULT_SNAP_PX = 8;

/** Auto-scroll starts this close (CSS px) to an edge of the lane area during a drag. */
export const AUTO_SCROLL_EDGE_PX = 32;

/** Fastest auto-scroll step, CSS px per frame (at or past the edge). */
export const AUTO_SCROLL_MAX_PX = 24;

/** The length a still gets when dropped (the engine's `timeline.insert` default, INV-25). */
export const STILL_DROP_DURATION = 5 * FLICKS_PER_SECOND;

/** Zoom tool: factor of a click (Alt+click divides by it). */
export const ZOOM_CLICK_FACTOR = 2;

/** Nudge with Shift: frames per press. */
export const NUDGE_FAR_FRAMES = 10;
