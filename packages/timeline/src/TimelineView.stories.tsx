// SPDX-License-Identifier: GPL-3.0-or-later
// Storybook stories for the Canvas2D timeline (picked up by the packages/ui Storybook and its
// visual harness, which renders every story in both themes). The canvas reads its colors from the
// design tokens of the active theme, so these stories double as the theme check. WP-2A.09's six
// visual states: Empty, MixedTracks, Selected, Locked, ZoomedOut and Markers (plus a deep zoom).
import type { MediaLoaders, WaveformPeaks } from "./cache.ts";
import {
  generateSmallTimeline,
  generateTimeline,
  syntheticLoaders,
  withMarkers,
} from "./fixtures.ts";
import type { TimelineItem, TimelineModel, TimelineTrack, ViewState } from "./model.ts";
import { TimelineCanvas } from "./react.tsx";
import { FLICKS_PER_SECOND as F } from "./time.ts";

export default {
  title: "Timeline/TimelineCanvas",
  component: TimelineCanvas,
  parameters: { layout: "fullscreen" },
};

/** Faces the canvas draws with, loaded before render so screenshots never catch a fallback. */
const loadFonts = async () => {
  await Promise.all([
    document.fonts.load('normal 500 11px "Instrument Sans"', "Aa0"),
    document.fonts.load('normal 500 12px "Instrument Sans"', "Aa0"),
    document.fonts.load('normal 400 12px "JetBrains Mono"', "0:"),
  ]);
  await document.fonts.ready;
  return {};
};

/**
 * Media made before render: one synthetic picture per media id and its peaks, served at once, so
 * the first frames after mount already show filmstrips and waveforms (stable screenshots).
 */
const loadMedia = async () => {
  const synthetic = syntheticLoaders();
  const ids = ["med_1", "med_2", "med_3", "med_4"];
  const signal = new AbortController().signal;
  const pictures = new Map<string, ImageBitmap>();
  const peaks = new Map<string, WaveformPeaks>();
  for (const mediaId of ids) {
    const tile = { mediaId, sourceTime: 0, tileDuration: F, width: 96, height: 54 };
    const bmp = await synthetic.filmstripTile?.(tile, signal);
    if (bmp) pictures.set(mediaId, bmp);
    const p = await synthetic.waveform?.({ mediaId, peaksPerSecond: 200 }, signal);
    if (p) peaks.set(mediaId, p);
  }
  const media: MediaLoaders = {
    filmstripTile: async (req) => {
      const bmp = pictures.get(req.mediaId);
      // The cache closes what it evicts: hand out copies of the pre-made picture.
      return bmp ? createImageBitmap(bmp) : null;
    },
    waveform: async (req) => peaks.get(req.mediaId) ?? null,
  };
  return { media };
};

type Loaded = { loaded: { media?: MediaLoaders } };

const frame = (height: number) =>
  ({
    height,
    margin: "var(--space-4)",
    border: "var(--border-thin) solid var(--border-subtle)",
    background: "var(--surface-1)",
  }) as const;

const s = (sec: number): number => Math.round(sec * F);

const emptyModel: TimelineModel = {
  fps: { num: 24, den: 1 },
  tracks: [
    { id: "trk_v1", name: "Video V1", kind: "media", size: "lg", main: true },
    { id: "trk_a1", name: "Audio A1", kind: "audio", size: "md" },
  ],
  items: [],
};

const item = (
  id: string,
  trackId: string,
  kind: TimelineItem["kind"],
  start: number,
  dur: number,
  label: string,
  extra: Partial<TimelineItem> = {},
): TimelineItem => ({ id, trackId, kind, start: s(start), duration: s(dur), label, ...extra });

/** Every track kind: titles, the main video, linked sound, a tall voice-over, collapsed captions. */
const mixedModel: TimelineModel = {
  fps: { num: 24, den: 1 },
  tracks: [
    { id: "t_g", name: "Titles", kind: "graphics", size: "md" },
    { id: "t_v", name: "Video V1", kind: "media", size: "lg", main: true },
    { id: "t_a", name: "Audio A1", kind: "audio", size: "md" },
    { id: "t_vo", name: "Voice-over", kind: "audio", height: 96 } satisfies TimelineTrack,
    { id: "t_c", name: "Captions", kind: "captions", size: "sm" },
    { id: "t_fx", name: "Adjust", kind: "effects", size: "md" },
  ],
  items: [
    item("g1", "t_g", "graphics", 1, 3, "Lower third: Ana Ruiz"),
    item("g2", "t_g", "graphics", 9, 2.5, "Title card"),
    item("v1", "t_v", "media", 0, 6, "interview_A.mov", {
      linkedGroup: "lg1",
      thumbnails: { mediaId: "med_1", sourceIn: 0 },
    }),
    item("v2", "t_v", "media", 6, 5, "broll_harbor_02.mp4", {
      thumbnails: { mediaId: "med_2", sourceIn: s(3) },
    }),
    item("v3", "t_v", "media", 11, 3, "Proposed: b-roll", { state: { proposed: true } }),
    item("a1", "t_a", "audio", 0, 6, "interview_A.mov", {
      linkedGroup: "lg1",
      waveform: { mediaId: "med_1", sourceIn: 0 },
    }),
    item("a2", "t_a", "audio", 6.5, 7, "music_bed.wav", {
      waveform: { mediaId: "med_4", sourceIn: 0 },
    }),
    item("vo", "t_vo", "audio", 2, 9, "voiceover_take3.wav", {
      waveform: { mediaId: "med_3", sourceIn: 0 },
    }),
    item("c1", "t_c", "captions", 0.5, 2.5, "So here is the thing"),
    item("c2", "t_c", "captions", 3.5, 3, "we tried it twice"),
    item("fx1", "t_fx", "effects", 2, 6, "Warm grade"),
    item("fx2", "t_fx", "effects", 9, 3, "Disabled blur", { state: { disabled: true } }),
  ],
};

const small = generateSmallTimeline();

/** The small project with each kind of track state: hidden video, muted audio, locked captions. */
const lockedModel: TimelineModel = {
  ...small,
  tracks: [
    { id: "trk_v1", name: "Video V1", kind: "media", size: "lg", main: true, hidden: true },
    { id: "trk_a1", name: "Audio A1", kind: "audio", size: "md", muted: true },
    { id: "trk_c1", name: "Captions", kind: "captions", size: "md", locked: true },
  ],
};

const stress = withMarkers(generateTimeline());

const markersModel: TimelineModel = {
  ...small,
  markers: [
    { id: "m1", t: s(1), label: "Intro", kind: "user" },
    { id: "m2", t: s(4.5), duration: s(2.5), label: "Fix the jump cut", kind: "user" },
    { id: "m3", t: s(9), label: "Chapter 2", kind: "chapter" },
    ...Array.from({ length: 16 }, (_, n) => ({
      id: `beat_${n}`,
      t: s(10 + n * 0.5),
      label: "",
      kind: "beat" as const,
    })),
  ],
};

const baseView: Partial<ViewState> = { pxPerSec: 80, playhead: 3 * F };

/** Track headers and an empty lane: ruler ticks, timecode labels and the playhead. */
export const Empty = {
  loaders: [loadFonts],
  render: () => (
    <div style={frame(220)}>
      <TimelineCanvas aria-label="Timeline" model={emptyModel} viewState={baseView} />
    </div>
  ),
};

/**
 * Every track kind with media: filmstrips, waveforms (a tall voice-over track), linked video and
 * sound with their badge, a collapsed captions track, an agent proposal and a disabled effect.
 */
export const MixedTracks = {
  loaders: [loadFonts, loadMedia],
  render: (_args: unknown, { loaded }: Loaded) => (
    <div style={frame(440)}>
      <TimelineCanvas
        aria-label="Timeline"
        model={mixedModel}
        {...(loaded.media && { loaders: loaded.media })}
        viewState={{ ...baseView, playhead: 5 * F }}
      />
    </div>
  ),
};

/** Selected clips with trim handles, snap guides by kind (edge, playhead, marker) and a marquee. */
export const Selected = {
  loaders: [loadFonts, loadMedia],
  render: (_args: unknown, { loaded }: Loaded) => (
    <div style={frame(300)}>
      <TimelineCanvas
        aria-label="Timeline"
        model={small}
        {...(loaded.media && { loaders: loaded.media })}
        viewState={{
          ...baseView,
          playhead: 7 * F,
          selection: new Set(["itm_2", "itm_5"]),
          snapGuides: [
            { kind: "edge", t: s(12) },
            { kind: "playhead", t: s(7) },
            { kind: "marker", t: s(14) },
          ],
          marquee: { x: 700, y: 150, w: 140, h: 70 },
        }}
      />
    </div>
  ),
};

/** A hidden video track, a muted audio track and a locked captions track. */
export const Locked = {
  loaders: [loadFonts, loadMedia],
  render: (_args: unknown, { loaded }: Loaded) => (
    <div style={frame(300)}>
      <TimelineCanvas
        aria-label="Timeline"
        model={lockedModel}
        {...(loaded.media && { loaders: loaded.media })}
        viewState={{ ...baseView, playhead: 8 * F }}
      />
    </div>
  ),
};

/** The S1 stress project zoomed out to its whole length: only visible clips are drawn. */
export const ZoomedOut = {
  loaders: [loadFonts],
  render: () => (
    <div style={frame(640)}>
      <TimelineCanvas
        aria-label="Timeline with 5,000 clips"
        model={stress}
        viewState={{ pxPerSec: 0.32, scrollX: 0, playhead: 1260 * F }}
      />
    </div>
  ),
};

/** Sequence markers (point, range, chapter, beats) and the in/out range. */
export const Markers = {
  loaders: [loadFonts],
  render: () => (
    <div style={frame(260)}>
      <TimelineCanvas
        aria-label="Timeline"
        model={markersModel}
        viewState={{ ...baseView, playhead: 6 * F, range: { in: 2 * F, out: 8 * F } }}
      />
    </div>
  ),
};

/** Zoomed in on the audio clip: waveform bars, per-frame ruler ticks, frame numbers. */
export const ZoomedWaveform = {
  loaders: [loadFonts, loadMedia],
  render: (_args: unknown, { loaded }: Loaded) => (
    <div style={frame(300)}>
      <TimelineCanvas
        aria-label="Timeline"
        model={small}
        {...(loaded.media && { loaders: loaded.media })}
        display={{ timeFormat: "frames" }}
        viewState={{ pxPerSec: 700, scrollX: Math.round(1.2 * F), playhead: Math.round(1.8 * F) }}
      />
    </div>
  ),
};
