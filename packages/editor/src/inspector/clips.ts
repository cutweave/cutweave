// SPDX-License-Identifier: GPL-3.0-or-later
// What the inspector inspects: the selected clips (with their sequence's frame rate) or one
// selected media asset. Selection is by ID (`viewStore.selection`); IDs that no longer exist in
// the document are dropped.

import type { MediaAsset, Project, SelectionRef } from "@cutweave/protocol";
import type { SelectedClip } from "./fields.ts";

/** The selected timeline clips, in selection order. */
export function selectSelectedClips(
  project: Project | null,
  selection: readonly SelectionRef[],
): SelectedClip[] {
  if (project === null) return [];
  const wanted = new Set<string>(selection.filter((r) => r.kind === "item").map((r) => r.id));
  if (wanted.size === 0) return [];
  const found = new Map<string, SelectedClip>();
  for (const sequence of Object.values(project.sequences)) {
    const fps = {
      num: sequence.format.frame_rate.num,
      den: sequence.format.frame_rate.den,
    };
    for (const track of Object.values(sequence.tracks)) {
      for (const item of Object.values(track.items)) {
        if (wanted.has(item.id) && item.kind !== "text") {
          found.set(item.id, {
            item,
            trackId: track.id,
            sequence: sequence.id,
            fps,
            locked: track.locked,
          });
        }
      }
    }
  }
  const ordered: SelectedClip[] = [];
  for (const ref of selection) {
    const clip = ref.kind === "item" ? found.get(ref.id) : undefined;
    if (clip !== undefined) ordered.push(clip);
  }
  return ordered;
}

/**
 * What the clip inspector shows and edits for a selection. A click on the timeline selects a
 * clip's whole linked group (timeline spec INV-341, WP-2A.10): the picture and its sound. The
 * inspector shows such a group as the one clip clicked (the first of it in the selection), as an
 * editor's clip inspector does; the engine still applies timing edits to every member (INV-35).
 * A group only partly selected (Alt+click, or linked selection off) keeps each selected member.
 */
export function selectInspectedClips(
  project: Project | null,
  selection: readonly SelectionRef[],
): SelectedClip[] {
  const clips = selectSelectedClips(project, selection);
  if (project === null || clips.length < 2) return clips;
  const wanted = new Set<string>(selection.filter((r) => r.kind === "item").map((r) => r.id));
  return oneClipPerGroup(project, clips, wanted);
}

function oneClipPerGroup(
  project: Project,
  clips: readonly SelectedClip[],
  wanted: ReadonlySet<string>,
): SelectedClip[] {
  const groups = new Set(
    clips.map((c) => c.item.linked_group).filter((g): g is NonNullable<typeof g> => g !== null),
  );
  if (groups.size === 0) return [...clips];
  /** Groups every member of which is selected. */
  const whole = new Set(groups);
  for (const sequence of Object.values(project.sequences)) {
    for (const track of Object.values(sequence.tracks)) {
      for (const item of Object.values(track.items)) {
        if (item.linked_group !== null && !wanted.has(item.id)) whole.delete(item.linked_group);
      }
    }
  }
  const seen = new Set<string>();
  return clips.filter((c) => {
    const group = c.item.linked_group;
    if (group === null || !whole.has(group)) return true;
    if (seen.has(group)) return false;
    seen.add(group);
    return true;
  });
}

/** The media asset the selection names, when it names exactly one and no clip. */
export function selectSelectedMedia(
  project: Project | null,
  selection: readonly SelectionRef[],
): MediaAsset | null {
  if (project === null || selection.some((r) => r.kind === "item")) return null;
  const media = selection.filter((r) => r.kind === "media");
  const only = media.length === 1 ? media[0] : undefined;
  return only === undefined ? null : (project.media[only.id as keyof typeof project.media] ?? null);
}
