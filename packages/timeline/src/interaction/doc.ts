// SPDX-License-Identifier: GPL-3.0-or-later
// Read-only lookups over a timeline projection, memoized per model object (a 5,000-item
// projection is indexed once per document patch, not once per pointer event).
import type { TimelineItem, TimelineModel, TimelineTrack } from "../model.ts";

interface DocIndex {
  readonly items: ReadonlyMap<string, TimelineItem>;
  readonly tracks: ReadonlyMap<string, TimelineTrack>;
  /** Linked group to its members, in model order. */
  readonly groups: ReadonlyMap<string, readonly string[]>;
  /** Track id to its items, sorted by start. */
  readonly byTrack: ReadonlyMap<string, readonly TimelineItem[]>;
}

const cache = new WeakMap<TimelineModel, DocIndex>();

function index(model: TimelineModel): DocIndex {
  const hit = cache.get(model);
  if (hit !== undefined) return hit;
  const items = new Map<string, TimelineItem>();
  const groups = new Map<string, string[]>();
  const byTrack = new Map<string, TimelineItem[]>();
  for (const it of model.items) {
    items.set(it.id, it);
    if (it.linkedGroup !== undefined) {
      const g = groups.get(it.linkedGroup);
      if (g === undefined) groups.set(it.linkedGroup, [it.id]);
      else g.push(it.id);
    }
    const list = byTrack.get(it.trackId);
    if (list === undefined) byTrack.set(it.trackId, [it]);
    else list.push(it);
  }
  for (const list of byTrack.values()) list.sort((a, b) => a.start - b.start);
  const tracks = new Map(model.tracks.map((t) => [t.id, t] as const));
  const built: DocIndex = { items, tracks, groups, byTrack };
  cache.set(model, built);
  return built;
}

export function itemOf(model: TimelineModel, id: string | null): TimelineItem | undefined {
  return id === null ? undefined : index(model).items.get(id);
}

export function trackOf(model: TimelineModel, id: string | null): TimelineTrack | undefined {
  return id === null ? undefined : index(model).tracks.get(id);
}

/** The items of a track, by start. */
export function itemsOn(model: TimelineModel, trackId: string): readonly TimelineItem[] {
  return index(model).byTrack.get(trackId) ?? [];
}

/** The members of `id`'s linked group, `id` first; just `[id]` when it is not linked. */
export function groupOf(model: TimelineModel, id: string): readonly string[] {
  const group = itemOf(model, id)?.linkedGroup;
  const members = group === undefined ? undefined : index(model).groups.get(group);
  if (members === undefined || members.length < 2) return [id];
  return [id, ...members.filter((m) => m !== id)];
}

export function isLocked(model: TimelineModel, trackId: string): boolean {
  return trackOf(model, trackId)?.locked === true;
}

/** The magnetic main track (spec §1.2). */
export function isMain(model: TimelineModel, trackId: string): boolean {
  return trackOf(model, trackId)?.main === true;
}

export const endOf = (it: TimelineItem): number => it.start + it.duration;

/** A clip that reads no bounded media (a still, a generator): nothing to slip (spec §4.12). */
export function isUnbounded(it: TimelineItem): boolean {
  const src = it.source;
  return src === undefined || src.still === true || src.mediaDuration === null;
}
