// SPDX-License-Identifier: GPL-3.0-or-later
// Match frame (WP-2A.14): the clip under the timeline's playhead, the position in its media the
// playhead shows, and the part of the media the clip plays. F opens that media in the Source
// view at that frame, with the clip's source range as its marks.
import type { ClipItem as Item, MediaId, Project } from "@cutweave/protocol";
import { selectActiveSequenceId } from "../state/selectors.ts";

export interface MatchFrame {
  readonly item: string;
  readonly media: MediaId;
  /** The position in the media of the frame under the playhead, flicks. */
  readonly at: number;
  /** The part of the media the clip plays: the marks the Source view gets. */
  readonly in: number;
  readonly out: number;
}

const speedOf = (clip: Item): number => {
  const speed = clip.speed;
  return speed !== undefined && speed.num > 0 && speed.den > 0 ? speed.num / speed.den : 1;
};

function matchOf(clip: Item, playhead: number): MatchFrame {
  const start = clip.source_in;
  // A freeze frame shows one position of its media for its whole length.
  if (clip.freeze != null) {
    return { item: clip.id, media: clip.media, at: start, in: start, out: start + clip.duration };
  }
  const speed = speedOf(clip);
  const span = Math.round(clip.duration * speed);
  const into = Math.round((playhead - clip.start) * speed);
  // A reversed clip plays its range backwards: its first frame is the range's last.
  const at = clip.reverse === true ? start + Math.max(0, span - into) : start + into;
  return { item: clip.id, media: clip.media, at, in: start, out: start + span };
}

/**
 * The clip match frame opens: of the media clips under `playhead`, a selected one first, then
 * the topmost picture, then sound. `null` when none is under the playhead.
 */
export function matchFrameAt(
  project: Project | null,
  playhead: number,
  selected: readonly string[] = [],
): MatchFrame | null {
  const id = selectActiveSequenceId(project);
  const sequence = id === null ? undefined : project?.sequences[id];
  if (sequence === undefined) return null;
  let best: { clip: Item; rank: number } | null = null;
  sequence.track_order.forEach((trackId, index) => {
    const track = sequence.tracks[trackId];
    if (track === undefined) return;
    for (const item of Object.values(track.items)) {
      if (item.kind !== "clip" || typeof item.media !== "string") continue;
      if (playhead < item.start || playhead >= item.start + item.duration) continue;
      // Selected first; then pictures before sound, each in the timeline's own order.
      const rank =
        (selected.includes(item.id) ? 0 : 1_000_000) + (track.kind === "audio" ? 1_000 : 0) + index;
      if (best === null || rank < best.rank) best = { clip: item, rank };
    }
  });
  const found = best as { clip: Item; rank: number } | null;
  return found === null ? null : matchOf(found.clip, playhead);
}
