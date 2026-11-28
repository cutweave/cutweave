// SPDX-License-Identifier: GPL-3.0-or-later
// What the surface tables read about the document when a menu opens (WP-2H.13): the clip, track
// and media a target names, found by ID in the engine mirror. Read-only: a menu never writes the
// mirror; it names a command and the engine's patch comes back.
import type { Clip, MediaAsset, MediaId, Sequence, SequenceId, Track } from "@cutweave/protocol";
import { docStore } from "../../state/docStore.ts";
import { mediaName, selectActiveSequenceId } from "../../state/selectors.ts";

export interface ActiveSequence {
  readonly id: SequenceId;
  readonly sequence: Sequence;
}

/** The sequence the timeline shows, or `null` without a project. */
export function activeSequence(): ActiveSequence | null {
  const project = docStore.getState().project;
  const id = selectActiveSequenceId(project);
  const sequence = id === null ? undefined : project?.sequences[id];
  return id === null || sequence === undefined ? null : { id, sequence };
}

export interface ClipFact {
  readonly clip: Clip;
  readonly track: Track;
}

/** The clip with this ID and the track that holds it. */
export function clipFact(id: string): ClipFact | null {
  const found = activeSequence();
  if (found === null) return null;
  for (const track of Object.values(found.sequence.tracks)) {
    const item = track.items[id as keyof typeof track.items];
    if (item !== undefined && item.kind !== "text") return { clip: item, track };
  }
  return null;
}

export function clipFacts(ids: readonly string[]): ClipFact[] {
  return ids.flatMap((id) => {
    const fact = clipFact(id);
    return fact === null ? [] : [fact];
  });
}

export interface TrackFact {
  readonly sequence: ActiveSequence;
  readonly track: Track;
  /** Position in the display order, 0 = top. */
  readonly index: number;
}

export function trackFact(id: string): TrackFact | null {
  const sequence = activeSequence();
  const track = sequence?.sequence.tracks[id as keyof Sequence["tracks"]];
  if (sequence === null || track === undefined) return null;
  return { sequence, track, index: sequence.sequence.track_order.indexOf(track.id) };
}

/** What the timeline calls the clip: its own name, else its media's file name. */
export function clipLabel(id: string): string {
  const fact = clipFact(id);
  if (fact === null) return "";
  return fact.clip.name ?? mediaName(mediaAsset(fact.clip.media) ?? undefined, fact.clip.media);
}

export function mediaAsset(id: string): MediaAsset | null {
  return docStore.getState().project?.media[id as MediaId] ?? null;
}

/** The file of a media asset as an absolute path, or `null` when it has none (a sequence). */
export function mediaPath(id: string): string | null {
  return mediaAsset(id)?.path_abs ?? null;
}

/** Where each animatable property lives in a clip (keyframes §7). */
export const PROP_PATHS: Readonly<Record<string, readonly string[]>> = {
  "transform.position": ["visual", "transform", "position"],
  "transform.scale": ["visual", "transform", "scale"],
  "transform.rotation": ["visual", "transform", "rotation"],
  "transform.anchor": ["visual", "transform", "anchor"],
  opacity: ["visual", "opacity"],
  "crop.left": ["visual", "crop", "left"],
  "crop.top": ["visual", "crop", "top"],
  "crop.right": ["visual", "crop", "right"],
  "crop.bottom": ["visual", "crop", "bottom"],
  "flip.horizontal": ["visual", "flip", "horizontal"],
  "flip.vertical": ["visual", "flip", "vertical"],
  audio_gain: ["audio", "gain"],
  audio_pan: ["audio", "pan"],
};

/** The keys of an animated property of the clip (item times, flicks), or `null` when static. */
export function keysOf(clip: Clip, prop: string): readonly { t: number }[] | null {
  let value: unknown = clip;
  for (const step of PROP_PATHS[prop] ?? []) {
    value = (value as Record<string, unknown> | null)?.[step];
  }
  return (value as { keyframes?: readonly { t: number }[] } | null)?.keyframes ?? null;
}

/**
 * The item time of the key of `prop` that sits exactly at the sequence time `at` (the playhead),
 * or `null` when the property is static or has no key there. Times are flicks, as stored.
 */
export function keyAtPlayhead(clip: Clip, prop: string, at: number): number | null {
  return keysOf(clip, prop)?.find((k) => clip.start + k.t === at)?.t ?? null;
}

/** The last time any clip of the sequence covers (where "Append to timeline" puts the next clip). */
export function sequenceEnd(sequence: Sequence): number {
  let end = 0;
  for (const track of Object.values(sequence.tracks)) {
    for (const item of Object.values(track.items)) end = Math.max(end, item.start + item.duration);
  }
  return end;
}
