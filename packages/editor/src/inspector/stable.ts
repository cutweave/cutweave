// SPDX-License-Identifier: GPL-3.0-or-later
// Keeping the inspector's parts from drawing again while a clip is dragged on the timeline
// (issue #244). A drag changes the clip's `start` (and so the document) on every update, but most
// of what the inspector shows does not depend on it: only the Start and Duration fields do, and
// the keyframe diamonds and "playhead is inside the clip" checks, which are told apart below.
// The parts that do not depend on it get the same `clips` and `project` values back until
// something they do show changes.

import type { Project } from "@cutweave/protocol";
import { deepEqual } from "@cutweave/ui";
import { useRef } from "react";
import { useViewStore, viewStore } from "../state/viewStore.ts";
import type { SelectedClip } from "./fields.ts";
import { animatedPaths } from "./video.ts";

/** Whether the playhead is inside each clip, as a string that changes when one of them flips. */
export function insideKey(clips: readonly SelectedClip[], playhead: number): string {
  return clips
    .map((c) => (playhead >= c.item.start && playhead < c.item.start + c.item.duration ? "1" : "0"))
    .join("");
}

/**
 * Whether `next` shows what `prev` showed for every part that is not the clip's timing: the same
 * clips, track and frame rate, and the same item but for its `start`. A start still matters to a
 * clip with keyframes (their times are relative to it) and to whether the playhead is inside.
 */
export function sameExceptStart(
  prev: readonly SelectedClip[],
  next: readonly SelectedClip[],
  playhead: number,
): boolean {
  if (prev.length !== next.length) return false;
  if (insideKey(prev, playhead) !== insideKey(next, playhead)) return false;
  return prev.every((a, i) => {
    const b = next[i];
    if (b === undefined) return false;
    if (a === b) return true;
    const moved = a.item.start !== b.item.start;
    return (
      a.trackId === b.trackId &&
      a.sequence === b.sequence &&
      a.locked === b.locked &&
      a.fps.num === b.fps.num &&
      a.fps.den === b.fps.den &&
      (!moved ||
        (animatedPaths(b.item).length === 0 && animatedPaths(b.item, "audio").length === 0)) &&
      deepEqual({ ...a.item, start: 0 }, { ...b.item, start: 0 })
    );
  });
}

/**
 * The selected clips as the parts that do not show their timing see them: the previous array
 * again while only `start` changed. It re-renders the caller when the playhead moves in or out of
 * a clip, for the clip as it is and as these parts last saw it.
 */
export function useClipsWithoutTiming(clips: readonly SelectedClip[]): readonly SelectedClip[] {
  const kept = useRef(clips);
  useViewStore((s) => `${insideKey(clips, s.playhead)}/${insideKey(kept.current, s.playhead)}`);
  if (
    kept.current !== clips &&
    !sameExceptStart(kept.current, clips, viewStore.getState().playhead)
  ) {
    kept.current = clips;
  }
  return kept.current;
}

/**
 * The project as the inspector's sections read it: the media table and the clips' tracks (their
 * name, flags and kind, not the items on them). It stays the same value while a clip moves.
 */
export function useProjectWithoutItems(
  project: Project | null,
  clips: readonly SelectedClip[],
): Project | null {
  const kept = useRef(project);
  const prev = kept.current;
  const same =
    prev === project ||
    (prev !== null &&
      project !== null &&
      prev.media === project.media &&
      clips.every((c) => {
        const a = prev.sequences[c.sequence]?.tracks[c.trackId];
        const b = project.sequences[c.sequence]?.tracks[c.trackId];
        return (
          a !== undefined && b !== undefined && deepEqual({ ...a, items: 0 }, { ...b, items: 0 })
        );
      }));
  if (!same) kept.current = project;
  return kept.current;
}
