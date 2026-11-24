// SPDX-License-Identifier: GPL-3.0-or-later
// What a media card shows, read from the engine document (the project mirror holds the probe
// results, so no `media.list` round trip is needed; `selectMedia` already reads the same data).
import type { MediaAsset, MediaId, Project } from "@cutweave/protocol";
import { FLICKS_PER_SECOND, type FrameRate } from "@cutweave/timeline";
import { mediaName } from "../state/selectors.ts";

export interface MediaCard {
  readonly id: MediaId;
  readonly kind: MediaAsset["kind"];
  readonly name: string;
  /** Flicks; `null` for stills. */
  readonly duration: number | null;
  /** Pixels of the main video stream, when there is one. */
  readonly width: number | null;
  readonly height: number | null;
  /** Frames per second, e.g. `29.97`; `null` when unknown. */
  readonly fps: number | null;
  /** The same rate as a ratio (for frame math); `null` when unknown. */
  readonly rate: FrameRate | null;
  /** Codec of the main stream (video, else audio), upper-cased: `H264`. */
  readonly codec: string | null;
  /** A file kind whose probe found no streams: nothing can be read from it. */
  readonly broken: boolean;
}

const cache = new WeakMap<Project, readonly MediaCard[]>();

function toCard(asset: MediaAsset): MediaCard {
  const video = asset.probe.streams.find((s) => s.kind === "video");
  const audio = asset.probe.streams.find((s) => s.kind === "audio");
  const fps = asset.probe.fps;
  const codec = (video ?? audio)?.codec ?? null;
  return {
    id: asset.id,
    kind: asset.kind,
    name: mediaName(asset, asset.id),
    duration: asset.probe.duration ?? null,
    width: video?.width ?? null,
    height: video?.height ?? null,
    fps: fps === null || fps.den === 0 ? null : Math.round((fps.num / fps.den) * 100) / 100,
    rate: fps === null || fps.den === 0 || fps.num === 0 ? null : { num: fps.num, den: fps.den },
    codec: codec === null ? null : codec.toUpperCase(),
    broken: asset.kind !== "sequence_ref" && asset.probe.streams.length === 0,
  };
}

/** The project's media as cards, ordered by ID; the same `Project` yields the same array. */
export function selectMediaCards(project: Project | null): readonly MediaCard[] {
  if (project === null) return [];
  let cards = cache.get(project);
  if (cards === undefined) {
    cards = Object.keys(project.media)
      .sort()
      .flatMap((key) => {
        const asset = project.media[key as MediaId];
        return asset === undefined ? [] : [toCard(asset)];
      });
    cache.set(project, cards);
  }
  return cards;
}

const usageCache = new WeakMap<Project, ReadonlyMap<string, number>>();

/** How many timeline clips use each asset (assets with none are absent). */
export function selectMediaUsage(project: Project | null): ReadonlyMap<string, number> {
  if (project === null) return new Map();
  let usage = usageCache.get(project);
  if (usage === undefined) {
    const counts = new Map<string, number>();
    for (const sequence of Object.values(project.sequences)) {
      for (const track of Object.values(sequence.tracks)) {
        for (const item of Object.values(track.items)) {
          counts.set(item.media, (counts.get(item.media) ?? 0) + 1);
        }
      }
    }
    usage = counts;
    usageCache.set(project, usage);
  }
  return usage;
}

/** `m:ss`, or `h:mm:ss` from an hour up; sub-second lengths show as `0:01`. */
export function formatDuration(flicks: number): string {
  const total = Math.max(0, Math.round(flicks / FLICKS_PER_SECOND));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}
