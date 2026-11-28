// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline interaction state machine (WP-2A.08). See `packages/timeline/README.md` for the
// state chart, the gestures and how the editor wires it (WP-2A.10).
export {
  AUTO_SCROLL_EDGE_PX,
  AUTO_SCROLL_INTENT_PX,
  AUTO_SCROLL_MAX_PX,
  DEFAULT_SNAP_PX,
  DRAG_THRESHOLD_PX,
  NUDGE_FAR_FRAMES,
  STILL_DROP_DURATION,
  ZOOM_CLICK_FACTOR,
} from "./constants.ts";
export { MACHINE_KEYS, type MachineKey, MODIFIER_KEYS, machineKeyAction } from "./keys.ts";
export {
  createTimelineMachine,
  type MachineListener,
  type TimelineMachine,
  type TimelineMachineDeps,
} from "./machine.ts";
export { type ChartEdge, type MachineTransition, transition, wantsFrame } from "./reducer.ts";
export { createSnapPort, type WasmSnapPort } from "./snap-port.ts";
export {
  DEFAULT_MACHINE_ENV,
  GESTURE_STATES,
  type GestureMode,
  initialMachineState,
  isMachineBusy,
  MACHINE_STATES,
  type MachineMode,
  type MachineState,
  openTx,
  type StateName,
} from "./state.ts";
export type {
  CancelReason,
  Cursor,
  DropPreview,
  DroppedMedia,
  EngineRefusal,
  FrameScheduler,
  GestureEdit,
  IntentType,
  KeyframeDragHandler,
  MachineAction,
  MachineCommand,
  MachineDeps,
  MachineEnv,
  MachineEvent,
  MachineIntent,
  Modifiers,
  NoticeCode,
  PointerPoint,
  SnapOutcome,
  SnapPort,
  SnapQuery,
  Tool,
} from "./types.ts";
export { TOOLS } from "./types.ts";
