// SPDX-License-Identifier: GPL-3.0-or-later
// Selectors over the engine document: what the timeline panel, the asset browser and the
// inspector read. Pure functions of `Project` (structurally shared by `applyPatch`, so a
// `WeakMap` keyed on the project object memoizes them for free).

import type {
  TrackKind as EngineTrackKind,
  Item,
  Marker,
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
  type TimelineMarker,
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

/** The active sequence's canvas, edit rate and length, for the inspector's Project section. */
export interface SequenceSummary {
  readonly width: number;
  readonly height: number;
  readonly frameRate: Sequence["format"]["frame_rate"];
  /** Flicks from 0 to the end of the last item on any track (0 when it is empty). */
  readonly duration: number;
}

/** The active sequence's summary, or `null` without a project or a sequence. */
export function selectSequenceSummary(project: Project | null): SequenceSummary | null {
  const id = selectActiveSequenceId(project);
  const sequence = id === null ? undefined : project?.sequences[id];
  if (sequence === undefined) {
    return null;
  }
  let duration = 0;
  for (const track of Object.values(sequence.tracks)) {
    for (const item of Object.values(track.items)) {
      duration = Math.max(duration, item.start + item.duration);
    }
  }
  const { width, height, frame_rate } = sequence.format;
  return { width, height, frameRate: frame_rate, duration };
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
    // The row height the user set (`track.set_height`); none is the default size.
    ...(track.height != null && { height: track.height }),
  };
}

function toMarker(marker: Marker): TimelineMarker {
  return {
    id: marker.id,
    t: marker.t,
    ...(marker.duration !== null && { duration: marker.duration }),
    label: marker.label,
    color: marker.color,
    kind: marker.kind,
  };
}

function toItem(item: Item, track: Track, project: Project): TimelineItem {
  const kind: ItemKind = kindOf(track.kind);
  const media = project.media[item.media];
  const label = item.name ?? mediaName(media, item.media);
  // The engine's trim bounds (`clip.trim`): stills and freeze frames have none, other media end
  // at the probed length when there is one.
  const still = media?.kind === "image" || (item.freeze ?? null) !== null;
  // WP-2D.01: speed (display only; the engine keeps the exact fraction) and direction. A freeze
  // frame shows one frame all along (speed 0).
  const speed =
    (item.freeze ?? null) !== null
      ? 0
      : item.speed === undefined
        ? 1
        : item.speed.num / item.speed.den;
  const reverse = item.reverse === true;
  const sourceIn = item.freeze?.at_source ?? item.source_in;
  const slice = {
    mediaId: item.media,
    sourceIn,
    ...(speed !== 1 && { speed }),
    ...(reverse && { reverse }),
  };
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
      ...(!still && speed !== 1 && { speed }),
      ...(!still && reverse && { reverse }),
    },
    ...(kind === "media" && { thumbnails: slice }),
    ...(kind === "audio" && { waveform: slice }),
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
  // Sequence markers, drawn in the ruler (by time, then ID, so the order is stable).
  const markers = Object.values(sequence.markers ?? {})
    .sort((a, b) => a.t - b.t || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
    .map(toMarker);
  return {
    fps: { num: sequence.format.frame_rate.num, den: sequence.format.frame_rate.den },
    tracks,
    items,
    ...(markers.length > 0 && { markers }),
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

/** Range objects per engine `in_out` object, so an unchanged in/out keeps its identity. */
const inOutCache = new WeakMap<object, { readonly in: number; readonly out: number }>();

/** The active sequence's in and out points as the timeline's range shade, or `null`. */
export function selectInOut(
  project: Project | null,
): { readonly in: number; readonly out: number } | null {
  const id = selectActiveSequenceId(project);
  const inOut = id === null ? null : (project?.sequences[id]?.in_out ?? null);
  if (inOut === null) return null;
  let range = inOutCache.get(inOut);
  if (range === undefined) {
    range = { in: inOut.start, out: inOut.end };
    inOutCache.set(inOut, range);
  }
  return range;
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
