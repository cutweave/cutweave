// @generated — do not edit; run cargo xtask gen
import type { AutosaveFailed, CommandPanicked, DocPatched, DocReset, HistoryChanged, JobInfo, MediaAnalysisFailed, MediaIndexed, ProjectClosed, ProjectDirty, ProjectSaved, ProjectStatus, RecoveryPayload, TxActivity, TxEnded } from "./types.ts";

/** Event name to payload (document events and engine events). */
export type EventMap = {
  "doc.command_panicked": CommandPanicked;
  "doc.patched": DocPatched;
  "doc.reset": DocReset;
  "edit.history_changed": HistoryChanged;
  "job.progress": JobInfo;
  "media.analysis_failed": MediaAnalysisFailed;
  "media.indexed": MediaIndexed;
  "project.autosave_failed": AutosaveFailed;
  "project.closed": ProjectClosed;
  "project.dirty": ProjectDirty;
  "project.opened": ProjectStatus;
  "project.recovery_available": RecoveryPayload;
  "project.saved": ProjectSaved;
  "tx.closed": TxEnded;
  "tx.locked": TxActivity;
  "tx.opened": TxActivity;
};
