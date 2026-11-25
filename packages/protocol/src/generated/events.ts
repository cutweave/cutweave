// @generated — do not edit; run cargo xtask gen
import type { AgentConnection, AutosaveFailed, CommandPanicked, DocPatched, DocReset, HistoryChanged, JobInfo, MediaAnalysisFailed, MediaIndexed, ProjectClosed, ProjectDirty, ProjectOpenFailed, ProjectOpening, ProjectSaved, ProjectStatus, RecoveryPayload, TxActivity, TxEnded } from "./types.ts";

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
  "project.connection_closed": AgentConnection;
  "project.connection_opened": AgentConnection;
  "project.dirty": ProjectDirty;
  "project.open_failed": ProjectOpenFailed;
  "project.opened": ProjectStatus;
  "project.opening": ProjectOpening;
  "project.recovery_available": RecoveryPayload;
  "project.saved": ProjectSaved;
  "tx.closed": TxEnded;
  "tx.locked": TxActivity;
  "tx.opened": TxActivity;
};
