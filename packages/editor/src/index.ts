// SPDX-License-Identifier: GPL-3.0-or-later
/** Package name; also the key the desktop shell uses to find this app. */
export const packageName = "@cutweave/editor";

export {
  createEngineStore,
  type EngineStoreState,
  engineStore,
  useConnectionState,
  useEngineReady,
  useEngineStore,
} from "./engine/engineStore.ts";
export {
  type CallStatus,
  type CommandName,
  type QueryName,
  runCommand,
  type UseCommand,
  type UseQuery,
  useCommand,
  useQuery,
} from "./engine/hooks.ts";
export {
  bridgeFromWindow,
  type EngineBridge,
  type EngineRuntime,
  type RuntimeOptions,
  type RuntimeStores,
  startEngineRuntime,
} from "./engine/runtime.ts";
export { createI18n, i18n, type TKey } from "./i18n/index.ts";
export { EditorShell } from "./shell/EditorShell.tsx";
export { createDocStore, type DocStore, docSink, docStore, useDocStore } from "./state/docStore.ts";
export {
  createJobStore,
  isActive,
  type JobStore,
  jobStore,
  selectActiveJobs,
  useJobStore,
} from "./state/jobStore.ts";
export {
  createPlaybackStore,
  type PlaybackStore,
  parsePlaybackState,
  playbackStore,
  usePlaybackStore,
} from "./state/playbackStore.ts";
export {
  type MediaEntry,
  selectActiveSequenceId,
  selectMedia,
  selectTimelineItems,
  selectTimelineModel,
} from "./state/selectors.ts";
export {
  createToastStore,
  MAX_NOTICES,
  type Notice,
  type ToastStore,
  toastStore,
  useToastStore,
} from "./state/toastStore.ts";
export {
  createViewStore,
  type ThemeId,
  useViewStore,
  type ViewStore,
  viewStore,
  type WorkspaceId,
} from "./state/viewStore.ts";
export { VIEW_SYNC_DEBOUNCE_MS, ViewSync } from "./state/viewSync.ts";
export {
  TimelineController,
  type TimelineControllerDeps,
  type TimelineGeometry,
  type TimelineOverlay,
} from "./timeline/controller.ts";
export {
  type FrameScheduler,
  type GestureOutcome,
  GestureSession,
  type GestureUpdate,
} from "./timeline/gesture.ts";
export { type TimelineIntents, useTimelineIntents } from "./timeline/useTimelineIntents.ts";
