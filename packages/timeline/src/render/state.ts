// SPDX-License-Identifier: GPL-3.0-or-later
// The renderer's per-frame state: one long-lived object the draw modules share, so a frame
// fills in fields and reuses buffers instead of allocating.
import type { SnapKindName } from "@cutweave/protocol/wasm";
import type { MediaCaches } from "../cache.ts";
import type { MutableRect } from "../geometry.ts";
import { IndexList } from "../interval-index.ts";
import type { TimelineLayout } from "../layout.ts";
import type { Rect, TimelineDisplay, ViewState } from "../model.ts";
import type { Theme } from "../theme.ts";
import type { FrameRate } from "../time.ts";
import type { Viewport } from "../viewport.ts";
import { GlyphCache } from "./glyphs.ts";
import { FittedLabels, TimeLabels } from "./text.ts";

export interface FrameStats {
  drawCalls: number;
  /** Clips that passed the per-track visibility query (what the index returned). */
  visibleItems: number;
  visibleKeyframes: number;
  /** Filmstrip tiles drawn from their own zoom level. */
  tiles: number;
  /** Tiles drawn from a neighbouring zoom level while their own one loads. */
  fallbackTiles: number;
  /** Tiles drawn as placeholders (nothing cached yet). */
  placeholders: number;
  labels: number;
  tracksDrawn: number;
  /** Sequence markers drawn in the ruler. */
  markers: number;
}

export interface FrameInput {
  readonly layout: TimelineLayout;
  readonly vp: Viewport;
  readonly theme: Theme;
  readonly fps: FrameRate;
  readonly dropFrame: boolean;
  readonly view: Pick<ViewState, "playhead" | "selection" | "snapGuides" | "range" | "marquee">;
  readonly caches: MediaCaches;
  readonly dpr: number;
  /** Item index under the pointer (trim handles show on hover), or -1. */
  readonly hoverItem: number;
  /** Redraw only this canvas-space rectangle (CSS px); null redraws everything. */
  readonly region: Rect | null;
  /** Display preferences (default: timecode). */
  readonly display?: TimelineDisplay;
}

/** Snap kinds in rank order, and how each draws its guide. */
export const GUIDE_KINDS: readonly SnapKindName[] = [
  "playhead",
  "edge",
  "marker",
  "beat",
  "in_out",
];

export interface GuideStyle {
  readonly color: string;
  readonly dashed: boolean;
}

export class RenderState {
  ctx!: CanvasRenderingContext2D;
  f!: FrameInput;
  readonly stats: FrameStats = {
    drawCalls: 0,
    visibleItems: 0,
    visibleKeyframes: 0,
    tiles: 0,
    fallbackTiles: 0,
    placeholders: 0,
    labels: 0,
    tracksDrawn: 0,
    markers: 0,
  };
  /** CSS px per flick at this frame's zoom. */
  k = 0;
  /** The repainted region's lane x range (canvas CSS px). */
  rx0 = 0;
  rx1 = 0;
  labelsLeft = 0;
  /** Half width of the playhead flag as last drawn; the hit-test uses it. */
  flagHalfWidth = 0;

  // Text metrics, measured once per theme (and after fonts load).
  metricsFor: Theme | null = null;
  charCaption = 6;
  charLabel = 7;
  timecodeWidth = 80;

  // Theme-derived constants, rebuilt with the metrics.
  dashGuide: number[] = [];
  dashProposal: number[] = [];
  guideStyle: GuideStyle[] = [];

  // Reused buffers.
  readonly ids = new IndexList();
  /** Item indices per (state group x kind): 3 groups x 5 kinds. */
  readonly buckets: IndexList[] = Array.from({ length: 15 }, () => new IndexList());
  readonly selected = new IndexList();
  /** The ruler's tick plan, filled in each frame. */
  readonly tickPlan = { step: 1, divisions: 1 };
  /** The repainted region of this frame (canvas CSS px). */
  readonly region: MutableRect = { x: 0, y: 0, w: 0, h: 0 };
  /** Scratch rectangle for geometry helpers. */
  readonly rect: MutableRect = { x: 0, y: 0, w: 0, h: 0 };
  readonly timeLabels = new TimeLabels();
  readonly itemLabels = new FittedLabels();
  readonly trackNames = new FittedLabels();
  readonly glyphs = new GlyphCache();

  resetStats(): void {
    const s = this.stats;
    s.drawCalls = s.visibleItems = s.visibleKeyframes = s.tiles = s.fallbackTiles = 0;
    s.placeholders = s.labels = s.tracksDrawn = s.markers = 0;
  }

  /** Measures text and rebuilds theme constants when the theme changed. */
  prepare(ctx: CanvasRenderingContext2D, th: Theme): void {
    if (this.metricsFor === th) return;
    this.metricsFor = th;
    const sample = "abcdefghijklmnopqrstuvwxyz0123456789";
    ctx.font = th.fontCaption;
    this.charCaption = ctx.measureText(sample).width / sample.length || 6;
    ctx.font = th.fontLabel;
    this.charLabel = ctx.measureText(sample).width / sample.length || 7;
    ctx.font = th.fontNumeric;
    this.timecodeWidth = ctx.measureText("00:00:00:00").width || 80;
    this.glyphs.clear();
    this.dashGuide = [th.space1, th.space1];
    this.dashProposal = [th.space1, th.space1];
    this.guideStyle = GUIDE_KINDS.map((kind) => guideStyle(kind, th));
  }

  /** Canvas x (CSS px) of time `t` at this frame's zoom and scroll. */
  x(t: number): number {
    const vp = this.f.vp;
    return vp.headerWidth + (t - vp.scrollX) * this.k;
  }
}

/** Snap guides by kind (tokens only): the "here" kinds solid marigold, the others dashed. */
function guideStyle(kind: SnapKindName, th: Theme): GuideStyle {
  switch (kind) {
    case "playhead":
      return { color: th.playhead, dashed: false };
    case "in_out":
      return { color: th.marigold, dashed: false };
    case "marker":
      return { color: th.marigoldText, dashed: true };
    case "beat":
      return { color: th.textMuted, dashed: true };
    default:
      return { color: th.textPrimary, dashed: true };
  }
}
