// SPDX-License-Identifier: GPL-3.0-or-later
// Deterministic synthetic projects and media for stories, tests and the benchmark. Seeded PRNG
// only (AGENTS.md: determinism). Not a product API: nothing in the editor imports this.
import type { MediaLoaders, WaveformPeaks } from "./cache.ts";
import type { ItemKind, TimelineItem, TimelineModel, TimelineTrack, TrackSize } from "./model.ts";
import { FLICKS_PER_SECOND, type FrameRate, frameToFlicks, snapToFrame } from "./time.ts";

/** Small fast seeded PRNG (mulberry32). */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface GenerateOptions {
  readonly seed?: number;
  readonly clipsPerTrack?: number;
  readonly keyframesPerClip?: number;
  readonly fps?: FrameRate;
  /** Distinct media ids the thumbnail/waveform refs cycle through. */
  readonly mediaCount?: number;
  /** Track layout; default is the S1 stress mix (20 tracks). */
  readonly kinds?: readonly ItemKind[];
}

const STRESS_KINDS: readonly ItemKind[] = [
  ...Array<ItemKind>(8).fill("media"),
  ...Array<ItemKind>(4).fill("audio"),
  ...Array<ItemKind>(3).fill("graphics"),
  ...Array<ItemKind>(3).fill("captions"),
  ...Array<ItemKind>(2).fill("effects"),
];

const KIND_LABEL: Readonly<Record<ItemKind, string>> = {
  media: "Video",
  audio: "Audio",
  graphics: "Title",
  captions: "Caption",
  effects: "Effect",
};
const KIND_PREFIX: Readonly<Record<ItemKind, string>> = {
  media: "V",
  audio: "A",
  graphics: "G",
  captions: "C",
  effects: "E",
};

/** The S1 stress project: 20 tracks x 250 clips = 5,000 clips, 10 keyframes each = 50,000. */
export function generateTimeline(opts: GenerateOptions = {}): TimelineModel {
  const rnd = mulberry32(opts.seed ?? 1);
  const fps = opts.fps ?? { num: 24, den: 1 };
  const kinds = opts.kinds ?? STRESS_KINDS;
  const clipsPerTrack = opts.clipsPerTrack ?? 250;
  const keyframes = opts.keyframesPerClip ?? 10;
  const mediaCount = opts.mediaCount ?? 200;
  const frame = (n: number): number => frameToFlicks(n, fps);

  const counts: Record<string, number> = {};
  const tracks: TimelineTrack[] = kinds.map((kind, i) => {
    counts[kind] = (counts[kind] ?? 0) + 1;
    const size: TrackSize = kind === "media" ? "lg" : kind === "audio" ? "md" : "sm";
    return {
      id: `trk_${i}`,
      name: `${KIND_LABEL[kind]} ${KIND_PREFIX[kind]}${counts[kind]}`,
      kind,
      size,
      main: i === 0,
    };
  });

  const items: TimelineItem[] = [];
  tracks.forEach((track, ti) => {
    let cursor = frame(Math.floor(rnd() * 120));
    for (let c = 0; c < clipsPerTrack; c++) {
      const len = frame(48 + Math.floor(rnd() * 24 * 22)); // 2 s .. 24 s
      const kf: number[] = [];
      for (let k = 0; k < keyframes; k++) {
        kf.push(snapToFrame(cursor + Math.floor(rnd() * len), fps, "floor"));
      }
      kf.sort((a, b) => a - b);
      const n = ti * clipsPerTrack + c;
      const media = `med_${n % mediaCount}`;
      const item: TimelineItem = {
        id: `itm_${n}`,
        trackId: track.id,
        kind: track.kind,
        start: cursor,
        duration: len,
        label: `${KIND_LABEL[track.kind]} clip ${n}`,
        keyframes: kf,
        ...(track.kind === "media" && { thumbnails: { mediaId: media, sourceIn: 0 } }),
        ...(track.kind === "audio" && { waveform: { mediaId: media, sourceIn: 0 } }),
        ...(rnd() < 0.01 && { state: { disabled: true } }),
      };
      items.push(item);
      cursor += len + (rnd() < 0.3 ? 0 : frame(Math.floor(rnd() * 72)));
    }
  });
  return { fps, tracks, items };
}

/** A small hand-written project: 1 video + 1 audio + captions, a few clips, one proposal. */
export function generateSmallTimeline(): TimelineModel {
  const fps: FrameRate = { num: 24, den: 1 };
  const s = (sec: number): number => Math.round(sec * FLICKS_PER_SECOND);
  const tracks: TimelineTrack[] = [
    { id: "trk_v1", name: "Video V1", kind: "media", size: "lg", main: true },
    { id: "trk_a1", name: "Audio A1", kind: "audio", size: "md", muted: true },
    { id: "trk_c1", name: "Captions", kind: "captions", size: "sm", locked: true },
  ];
  const items: TimelineItem[] = [
    {
      id: "itm_1",
      trackId: "trk_v1",
      kind: "media",
      start: s(0.5),
      duration: s(6),
      label: "interview_A.mov",
      thumbnails: { mediaId: "med_1", sourceIn: 0 },
      keyframes: [s(2), s(4.5)],
    },
    {
      id: "itm_2",
      trackId: "trk_v1",
      kind: "media",
      start: s(7),
      duration: s(5),
      label: "broll_harbor_02.mp4",
      thumbnails: { mediaId: "med_2", sourceIn: s(3) },
      state: { selected: true },
    },
    {
      id: "itm_3",
      trackId: "trk_v1",
      kind: "media",
      start: s(12.5),
      duration: s(4),
      label: "Proposed: b-roll",
      state: { proposed: true },
    },
    {
      id: "itm_4",
      trackId: "trk_a1",
      kind: "audio",
      start: s(0.5),
      duration: s(11.5),
      label: "interview_A.wav",
      waveform: { mediaId: "med_1", sourceIn: 0 },
    },
    {
      id: "itm_5",
      trackId: "trk_c1",
      kind: "captions",
      start: s(1),
      duration: s(3),
      label: "So here is the thing",
    },
    {
      id: "itm_6",
      trackId: "trk_c1",
      kind: "captions",
      start: s(5),
      duration: s(2.5),
      label: "disabled caption",
      state: { disabled: true },
    },
  ];
  return { fps, tracks, items };
}

/** Pixel size of a synthetic tile; real loaders return whatever the engine serves. */
const TILE_W = 96;
const TILE_H = 54;

/**
 * Synthetic loaders for stories and the benchmark: tiles are rendered with `OffscreenCanvas`
 * (a hue per media id, a marker per tile index) and waveforms are seeded noise. They resolve on a
 * microtask, standing in for the engine's `/media/{id}/filmstrip` and `/waveform` endpoints.
 */
export function syntheticLoaders(): MediaLoaders {
  const hash = (s: string): number => {
    let h = 2166136261;
    for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
    return h >>> 0;
  };
  return {
    async filmstripTile(req) {
      const hue = hash(req.mediaId) % 360;
      const shade = hash(`${req.mediaId}|${req.sourceTime}`) % 20;
      const c = new OffscreenCanvas(TILE_W, TILE_H);
      const g = c.getContext("2d");
      if (!g) return null;
      g.fillStyle = `hsl(${hue} 35% ${38 + shade}%)`;
      g.fillRect(0, 0, TILE_W, TILE_H);
      g.fillStyle = `hsl(${hue} 40% 20%)`;
      g.fillRect(0, TILE_H * 0.62, TILE_W, TILE_H * 0.38);
      return createImageBitmap(c);
    },
    async waveform(req) {
      const rnd = mulberry32(hash(req.mediaId));
      const pps = Math.min(req.peaksPerSecond, 500); // loaders may answer at another resolution
      const seconds = 300;
      const n = Math.ceil(seconds * pps);
      const min = new Float32Array(n);
      const max = new Float32Array(n);
      let env = 0.3;
      for (let i = 0; i < n; i++) {
        env = Math.min(1, Math.max(0.05, env + (rnd() - 0.5) * 0.12));
        const a = env * (0.5 + 0.5 * rnd());
        max[i] = a;
        min[i] = -a * (0.7 + 0.3 * rnd());
      }
      const peaks: WaveformPeaks = { peaksPerSecond: pps, min, max };
      return peaks;
    },
  };
}
