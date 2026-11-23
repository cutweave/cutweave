// @generated — do not edit; run cargo xtask gen
import type { CommandPanicked, DocPatched, DocReset, HistoryChanged, TxActivity, TxEnded } from "./types.ts";

/** Event name to payload. Engine events are `unknown` until their payload types are exported. */
export type EventMap = {
  "doc.command_panicked": CommandPanicked;
  "doc.patched": DocPatched;
  "doc.reset": DocReset;
  "edit.history_changed": HistoryChanged;
  // TODO: `AutosaveFailed` is not exported by specta yet (engine event).
  "project.autosave_failed": unknown;
  // TODO: `ProjectDirty` is not exported by specta yet (engine event).
  "project.dirty": unknown;
  // TODO: `RecoveryPayload` is not exported by specta yet (engine event).
  "project.recovery_available": unknown;
  // TODO: `ProjectSaved` is not exported by specta yet (engine event).
  "project.saved": unknown;
  "tx.closed": TxEnded;
  "tx.locked": TxActivity;
  "tx.opened": TxActivity;
};
