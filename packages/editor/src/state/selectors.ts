// SPDX-License-Identifier: GPL-3.0-or-later
// Selectors over the engine document: what the timeline panel, the asset browser and the
// inspector read. Pure functions of `Project` (structurally shared by `applyPatch`, so a
// `WeakMap` keyed on the project object memoizes them for free).

import type {
  TrackKind as EngineTrackKind,
  Item,
  MediaAsset,
  MediaId,
  Project,
  Sequence,
  SequenceId,
  Track,
} from "@cutweave/protocol";
import {
  EMPTY_MODEL,
  type ItemKind,
  type TimelineItem,
  type TimelineModel,
  type TimelineTrack,
  type TrackKind,
} from "@cutweave/timeline";

/** The sequence the editor shows: v1 projects have one; with several, the lowest ID. */
export function selectActiveSequenceId(project: Project | null): SequenceId | null {
  if (project === null) {
    return null;
  }
  const ids = Object.keys(project.sequences).sort();
  return (ids[0] as SequenceId | undefined) ?? null;
}

const TRACK_KIND: Readonly<Record<EngineTrackKind, TrackKind>> = {
  video: "media",
  audio: "audio",
  text: "graphics",
  caption: "captions",
  adjustment: "effects",
};

const kindOf = (track: EngineTrackKind): TrackKind => TRACK_KIND[track] ?? "media";

/** The file name of a media asset, or its ID when it has no path. */
export function mediaName(media: MediaAsset | undefined, id: MediaId): string {
  const path = media?.path_rel ?? media?.path_abs ?? null;
  if (path === null) {
    return id;
  }
  return path.split(/[\\/]/).pop() ?? id;
}

function toTrack(track: Track): TimelineTrack {
  return {
    id: track.id,
    name: track.name,
    kind: kindOf(track.kind),
    muted: track.muted,
    locked: track.locked,
    hidden: track.hidden,
    main: track.magnetic,
  };
}

function toItem(item: Item, track: Track, project: Project): TimelineItem {
  const kind: ItemKind = kindOf(track.kind);
  const media = project.media[item.media];
  const label = item.name ?? mediaName(media, item.media);
  // The engine's trim bounds (`clip.trim`): stills have none, other media end at the probed
  // length when there is one.
  const still = media?.kind === "image";
  return {
    id: item.id,
    trackId: track.id,
    kind,
    start: item.start,
    duration: item.duration,
    label,
    ...(item.linked_group !== null && { linkedGroup: item.linked_group }),
    source: {
      sourceIn: item.source_in,
      mediaDuration: still ? null : (media?.probe.duration ?? null),
      ...(still && { still }),
    },
    ...(kind === "media" && { thumbnails: { mediaId: item.media, sourceIn: item.source_in } }),
    ...(kind === "audio" && { waveform: { mediaId: item.media, sourceIn: item.source_in } }),
    ...(!item.enabled && { state: { disabled: true } }),
  };
}

function build(sequence: Sequence, project: Project): TimelineModel {
  const tracks: TimelineTrack[] = [];
  const items: TimelineItem[] = [];
  // `track_order` is top to bottom, which is how the timeline lists lanes.
  for (const id of sequence.track_order) {
    const track = sequence.tracks[id];
    if (track === undefined) {
      continue;
    }
    tracks.push(toTrack(track));
    const own = Object.values(track.items).sort((a, b) => a.start - b.start);
    for (const item of own) {
      items.push(toItem(item, track, project));
    }
  }
  return {
    fps: { num: sequence.format.frame_rate.num, den: sequence.format.frame_rate.den },
    tracks,
    items,
  };
}

const cache = new WeakMap<Project, Map<SequenceId, TimelineModel>>();

/**
 * The timeline renderer's model for one sequence (default: the active one): tracks top to
 * bottom and every item by ID. An empty model when there is no project or sequence. The same
 * `Project` object always yields the same model object.
 */
export function selectTimelineModel(
  project: Project | null,
  sequenceId: SequenceId | null = selectActiveSequenceId(project),
): TimelineModel {
  if (project === null || sequenceId === null) {
    return EMPTY_MODEL;
  }
  const sequence = project.sequences[sequenceId];
  if (sequence === undefined) {
    return EMPTY_MODEL;
  }
  let perSequence = cache.get(project);
  if (perSequence === undefined) {
    perSequence = new Map();
    cache.set(project, perSequence);
  }
  let model = perSequence.get(sequenceId);
  if (model === undefined) {
    model = build(sequence, project);
    perSequence.set(sequenceId, model);
  }
  return model;
}

/** The items of the active sequence, in timeline order per track. */
export function selectTimelineItems(project: Project | null): readonly TimelineItem[] {
  return selectTimelineModel(project).items;
}

export interface MediaEntry {
  readonly id: MediaId;
  readonly kind: MediaAsset["kind"];
  readonly name: string;
  /** Flicks; `null` for stills. */
  readonly duration: number | null;
}

const mediaCache = new WeakMap<Project, readonly MediaEntry[]>();

/** The project's media for the asset browser, ordered by ID. */
export function selectMedia(project: Project | null): readonly MediaEntry[] {
  if (project === null) {
    return [];
  }
  let entries = mediaCache.get(project);
  if (entries === undefined) {
    entries = Object.keys(project.media)
      .sort()
      .flatMap((key) => {
        const asset = project.media[key as MediaId];
        return asset === undefined
          ? []
          : [
              {
                id: asset.id,
                kind: asset.kind,
                name: mediaName(asset, asset.id),
                duration: asset.probe.duration ?? null,
              },
            ];
      });
    mediaCache.set(project, entries);
  }
  return entries;
}
