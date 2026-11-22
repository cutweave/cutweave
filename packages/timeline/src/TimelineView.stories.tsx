// SPDX-License-Identifier: GPL-3.0-or-later
// Storybook stories for the Canvas2D timeline (picked up by the packages/ui Storybook and its
// visual harness, which renders every story in both themes). The canvas reads its colors from the
// design tokens of the active theme, so these stories double as the theme check.
import { generateSmallTimeline, generateTimeline, syntheticLoaders } from "./fixtures.ts";
import type { TimelineModel, ViewState } from "./model.ts";
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

const frame = (height: number) =>
  ({
    height,
    margin: "var(--space-4)",
    border: "var(--border-thin) solid var(--border-subtle)",
    background: "var(--surface-1)",
  }) as const;

const loaders = syntheticLoaders();

const emptyModel: TimelineModel = {
  fps: { num: 24, den: 1 },
  tracks: [
    { id: "trk_v1", name: "Video V1", kind: "media", size: "lg", main: true },
    { id: "trk_a1", name: "Audio A1", kind: "audio", size: "md" },
  ],
  items: [],
};

const small = generateSmallTimeline();
const stress = generateTimeline();

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

/** Three media clips with filmstrip tiles (synthetic loaders), plus audio, captions and a proposal. */
export const Filmstrips = {
  loaders: [loadFonts],
  render: () => (
    <div style={frame(300)}>
      <TimelineCanvas
        aria-label="Timeline"
        model={small}
        loaders={loaders}
        viewState={{ ...baseView, playhead: 5 * F, snapGuides: [7 * F] }}
      />
    </div>
  ),
};

/** Zoomed in on the audio clip: waveform bars, per-frame ruler ticks. */
export const ZoomedWaveform = {
  loaders: [loadFonts],
  render: () => (
    <div style={frame(300)}>
      <TimelineCanvas
        aria-label="Timeline"
        model={small}
        loaders={loaders}
        viewState={{ pxPerSec: 700, scrollX: Math.round(1.2 * F), playhead: Math.round(1.8 * F) }}
      />
    </div>
  ),
};

/** Selected clip with trim handles, disabled and proposed clips, a locked and a muted track. */
export const States = {
  loaders: [loadFonts],
  render: () => (
    <div style={frame(300)}>
      <TimelineCanvas
        aria-label="Timeline"
        model={small}
        viewState={{
          ...baseView,
          playhead: 8 * F,
          selection: new Set(["itm_2", "itm_5"]),
          range: { in: 2 * F, out: 6 * F },
        }}
      />
    </div>
  ),
};

/** The S1 stress project: 20 tracks, 5,000 clips, 50,000 keyframes; only visible clips are drawn. */
export const Stress5000Clips = {
  loaders: [loadFonts],
  render: () => (
    <div style={frame(640)}>
      <TimelineCanvas
        aria-label="Timeline with 5,000 clips"
        model={stress}
        viewState={{ pxPerSec: 6, scrollX: 1200 * F, playhead: 1260 * F }}
      />
    </div>
  ),
};
