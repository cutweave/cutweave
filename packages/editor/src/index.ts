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
export { InspectorEditor, type InspectorEditorDeps } from "./inspector/editor.ts";
export {
  FIELDS,
  type FieldDef,
  type FieldId,
  type SelectedClip,
} from "./inspector/fields.ts";
export { type DesktopBridge, desktopFromWindow } from "./session/desktop.ts";
export { historyLabel } from "./session/historyLabels.ts";
export { ProjectController, type ProjectControllerDeps } from "./session/projectController.ts";
export {
  createSessionStore,
  type SessionStore,
  sessionStore,
  type UndoStacks,
  useSessionStore,
} from "./session/sessionStore.ts";
export { SessionSync } from "./session/sessionSync.ts";
export {
  createSessionUiStore,
  type SessionUiStore,
  sessionUiStore,
} from "./session/sessionUiStore.ts";
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
  useViewStore,
  type ViewStore,
  viewStore,
  type WorkspaceId,
} from "./state/viewStore.ts";
export { VIEW_SYNC_DEBOUNCE_MS, ViewSync } from "./state/viewSync.ts";
export { type ParsedTime, parseTimeInput, type TimeInputError } from "./time/timeInput.ts";
export {
  type FrameScheduler,
  type GestureOutcome,
  GestureSession,
  type GestureUpdate,
} from "./timeline/gesture.ts";
export { EngineQueue, GestureTransactions } from "./timeline/gestureTx.ts";
export {
  type TimelineGeometry,
  TimelineHost,
  type TimelineHostDeps,
  type TimelineUi,
} from "./timeline/host.ts";
export { RTT_MEASURE, RttProbe } from "./timeline/rttProbe.ts";
export { type TimelineBinding, useTimelineHost } from "./timeline/useTimelineHost.ts";
