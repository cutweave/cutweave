// SPDX-License-Identifier: GPL-3.0-or-later
// What the kinds of media have in common for the UI (ADR-0035: an image sequence is video for
// tracks, playback, proxies and export; it only has no sound).
import type { MediaKind } from "@cutweave/protocol";

/** Media with pictures over time: a video file, or an image sequence. */
export const isMovingPicture = (kind: MediaKind | undefined): boolean =>
  kind === "video" || kind === "image_sequence";
