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
export {
  bucketPxPerSec,
  FILMSTRIP_BYTES,
  LoadingCache,
  MediaCaches,
  WAVEFORM_BYTES,
  zoomBucket,
} from "./cache.ts";
export {
  generateSmallTimeline,
  generateTimeline,
  mulberry32,
  syntheticLoaders,
  withMarkers,
} from "./fixtures.ts";
export type { HeaderToggle } from "./geometry.ts";
export type { HitContext, HitResult, HitZone } from "./hit-test.ts";
export { hitTest } from "./hit-test.ts";
export type {
  DroppingMediaMode,
  HoveringMode,
  IdleMode,
  Intent,
  InteractionEnv,
  InteractionEvent,
  InteractionState,
  InteractionStateName,
  KeyInput,
  MediaDrag,
  MediaDragPoint,
  Mode,
  MovingMode,
  PanningMode,
  PointerInput,
  ScrubbingMode,
  SelectingMode,
  Transition,
  TrimmingMode,
} from "./interaction.ts";
export {
  DEFAULT_ENV,
  DRAG_THRESHOLD_PX,
  fromPointerEvent,
  initialState,
  isBusy,
  isGestureOpen,
  reduce,
  STILL_DROP_DURATION,
  TIMELINE_KEYS,
  type TimelineKeyAction,
  timelineKeyAction,
  trimLimits,
} from "./interaction.ts";
export { IntervalIndex } from "./interval-index.ts";
export type { TrackMetrics } from "./layout.ts";
export { TimelineLayout } from "./layout.ts";
export type {
  ItemKind,
  ItemState,
  MarkerKind,
  MediaSlice,
  Rect,
  SnapGuide,
  SourceRange,
  TimeFormat,
  TimelineDisplay,
  TimelineItem,
  TimelineMarker,
  TimelineModel,
  TimelineTrack,
  TrackKind,
  TrackSize,
  ViewState,
} from "./model.ts";
export { DEFAULT_DISPLAY, DEFAULT_VIEW_STATE, EMPTY_MODEL } from "./model.ts";
export type { TimelineCanvasProps } from "./react.tsx";
export { TimelineCanvas } from "./react.tsx";
export { tileSpans } from "./render/media.ts";
export { TimelineRenderer } from "./render/renderer.ts";
export type { FrameInput, FrameStats } from "./render/state.ts";
export type { TickPlan } from "./ruler.ts";
export { chooseTickPlan, tickCandidates } from "./ruler.ts";
export type {
  SharedSnapResult,
  SnapCandidate,
  SnapIndex,
  SnapKindName,
  SnapRequest,
  SnapSources,
} from "./snap/index.ts";
export { createSnapIndex, SNAP_KINDS, snap, snapCandidates } from "./snap/index.ts";
export type { SnapResult, SnapTargetOptions } from "./snap.ts";
export { nearestTarget, SNAP_PX, snapSpan, snapTargets, snapTolerance } from "./snap.ts";
export type { ClipColors, Theme } from "./theme.ts";
export { readTheme } from "./theme.ts";
export type { FrameRate } from "./time.ts";
export {
  FLICKS_PER_SECOND,
  flicksToFrame,
  formatFrameTimecode,
  formatTimecode,
  frameDurationFlicks,
  frameToFlicks,
  snapToFrame,
} from "./time.ts";
export type { TimelinePointerEvent, TimelineViewOptions } from "./timeline-view.ts";
export { LONG_PRESS_MS, LONG_PRESS_SLOP_PX, TimelineView } from "./timeline-view.ts";
export type { Viewport } from "./viewport.ts";
export {
  clampScroll,
  clampZoom,
  durationToPx,
  fitRange,
  flicksPerPx,
  MAX_PX_PER_SEC,
  MIN_PX_PER_SEC,
  timeToX,
  visibleTimeRange,
  xToTime,
  zoomAround,
} from "./viewport.ts";
