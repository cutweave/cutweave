// SPDX-License-Identifier: GPL-3.0-or-later
/** Package name. */
export const packageName = "@cutweave/timeline";

export type {
  FilmstripRequest,
  LoadingCacheOptions,
  MediaCacheOptions,
  MediaLoaders,
  WaveformPeaks,
  WaveformRequest,
} from "./cache.ts";
export { bucketPxPerSec, LoadingCache, MediaCaches, zoomBucket } from "./cache.ts";
export {
  generateSmallTimeline,
  generateTimeline,
  mulberry32,
  syntheticLoaders,
} from "./fixtures.ts";
export type { HeaderToggle } from "./geometry.ts";
export type { HitContext, HitResult, HitZone } from "./hit-test.ts";
export { hitTest } from "./hit-test.ts";
export { IntervalIndex } from "./interval-index.ts";
export type { TrackMetrics } from "./layout.ts";
export { TimelineLayout } from "./layout.ts";
export type {
  ItemKind,
  ItemState,
  MediaSlice,
  Rect,
  TimelineItem,
  TimelineModel,
  TimelineTrack,
  TrackKind,
  TrackSize,
  ViewState,
} from "./model.ts";
export { DEFAULT_VIEW_STATE, EMPTY_MODEL } from "./model.ts";
export type { TimelineCanvasProps } from "./react.tsx";
export { TimelineCanvas } from "./react.tsx";
export type { FrameInput, FrameStats } from "./renderer.ts";
export { TimelineRenderer } from "./renderer.ts";
export type { TickPlan } from "./ruler.ts";
export { chooseTickPlan, tickCandidates } from "./ruler.ts";
export type { ClipColors, Theme } from "./theme.ts";
export { readTheme } from "./theme.ts";
export type { FrameRate } from "./time.ts";
export {
  FLICKS_PER_SECOND,
  flicksToFrame,
  formatTimecode,
  frameDurationFlicks,
  frameToFlicks,
  snapToFrame,
} from "./time.ts";
export type { TimelinePointerEvent, TimelineViewOptions } from "./timeline-view.ts";
export { TimelineView } from "./timeline-view.ts";
export type { Viewport } from "./viewport.ts";
export {
  clampScroll,
  clampZoom,
  durationToPx,
  flicksPerPx,
  MAX_PX_PER_SEC,
  MIN_PX_PER_SEC,
  timeToX,
  visibleTimeRange,
  xToTime,
  zoomAround,
} from "./viewport.ts";
