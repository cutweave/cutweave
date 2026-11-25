// @generated — do not edit; run cargo xtask gen
import type { BeginTx, CancelTx, Checkpoint, CheckpointInfo, CheckpointList, ClipSplit, ClipSplitOutput, ClipTrim, ClipTrimOutput, CommitTx, DeleteCheckpoint, ExportStart, ExportStartOutput, GetHistory, HistoryMoved, HistoryPage, ItemDelete, ItemDeleteOutput, ItemGet, ItemGetOutput, ItemMove, ItemMoveOutput, ItemSetProps, ItemSetPropsOutput, JobCancel, JobInfo, JobList, JobListOutput, JobStatus, JobWait, ListCheckpoints, ListLocks, LockList, MediaCacheStatus, MediaCacheStatusOutput, MediaCapabilities, MediaCapabilitiesOutput, MediaClearCache, MediaClearCacheOutput, MediaDeleteProxy, MediaGenerateProxy, MediaImport, MediaImportOutput, MediaList, MediaListOutput, MediaProxyStatus, MediaRegister, MediaRegistered, MediaRemove, MediaRemoveOutput, MediaSetCacheConfig, MediaSetDecodePolicy, MediaSetDecodePolicyOutput, ProjectClose, ProjectCloseOutput, ProjectConnections, ProjectConnectionsOutput, ProjectDiscardRecovery, ProjectInfo, ProjectInfoOutput, ProjectNew, ProjectNewOutput, ProjectOpen, ProjectOpenOutput, ProjectRecover, ProjectRecoverOutput, ProjectRename, ProjectRenameOutput, ProjectReportFocus, ProjectReportFocusOutput, ProjectSave, ProjectSaveAs, ProjectSaveOutput, ProjectSetAllowedRoots, ProjectSetAllowedRootsOutput, ProjectSetAutosave, ProjectSetAutosaveOutput, ProxiesOutput, Redo, Restore, Restored, TimelineAddMarker, TimelineAddMarkerOutput, TimelineClearInOut, TimelineClearInOutOutput, TimelineGet, TimelineGetOutput, TimelineInsert, TimelineInsertOutput, TimelineListMarkers, TimelineListMarkersOutput, TimelineMoveMarker, TimelineMoveMarkerOutput, TimelineRemoveMarker, TimelineRemoveMarkerOutput, TimelineSetInOut, TimelineSetInOutOutput, TrackAdd, TrackAddOutput, TrackList, TrackListOutput, TrackRemove, TrackRemoveOutput, TrackReorder, TrackReorderOutput, TrackSet, TrackSetHeight, TrackSetHeightOutput, TrackSetMagnetic, TrackSetMagneticOutput, TrackSetOutput, TxCancelled, TxCommitted, TxOpened, UiState, Undo, ViewGet, ViewSelect, ViewSet, ViewSetPlayhead, ViewSetZoom } from "./types.ts";

/** Commands and engine commands: name to input and output. */
export type CommandMap = {
  "clip.split": { input: ClipSplit; output: ClipSplitOutput };
  "clip.trim": { input: ClipTrim; output: ClipTrimOutput };
  "edit.begin_tx": { input: BeginTx; output: TxOpened };
  "edit.cancel_tx": { input: CancelTx; output: TxCancelled };
  "edit.checkpoint": { input: Checkpoint; output: CheckpointInfo };
  "edit.commit_tx": { input: CommitTx; output: TxCommitted };
  "edit.delete_checkpoint": { input: DeleteCheckpoint; output: CheckpointInfo };
  "edit.history": { input: GetHistory; output: HistoryPage };
  "edit.list_checkpoints": { input: ListCheckpoints; output: CheckpointList };
  "edit.locks": { input: ListLocks; output: LockList };
  "edit.redo": { input: Redo; output: HistoryMoved };
  "edit.restore": { input: Restore; output: Restored };
  "edit.undo": { input: Undo; output: HistoryMoved };
  "export.start": { input: ExportStart; output: ExportStartOutput };
  "item.delete": { input: ItemDelete; output: ItemDeleteOutput };
  "item.move": { input: ItemMove; output: ItemMoveOutput };
  "item.set_props": { input: ItemSetProps; output: ItemSetPropsOutput };
  "job.cancel": { input: JobCancel; output: JobInfo };
  "job.list": { input: JobList; output: JobListOutput };
  "job.status": { input: JobStatus; output: JobInfo };
  "job.wait": { input: JobWait; output: JobInfo };
  "media.cache_status": { input: MediaCacheStatus; output: MediaCacheStatusOutput };
  "media.capabilities": { input: MediaCapabilities; output: MediaCapabilitiesOutput };
  "media.clear_cache": { input: MediaClearCache; output: MediaClearCacheOutput };
  "media.delete_proxy": { input: MediaDeleteProxy; output: ProxiesOutput };
  "media.generate_proxy": { input: MediaGenerateProxy; output: ProxiesOutput };
  "media.import": { input: MediaImport; output: MediaImportOutput };
  "media.proxy_status": { input: MediaProxyStatus; output: ProxiesOutput };
  "media.register": { input: MediaRegister; output: MediaRegistered };
  "media.remove": { input: MediaRemove; output: MediaRemoveOutput };
  "media.set_cache_config": { input: MediaSetCacheConfig; output: MediaCacheStatusOutput };
  "media.set_decode_policy": { input: MediaSetDecodePolicy; output: MediaSetDecodePolicyOutput };
  "project.close": { input: ProjectClose; output: ProjectCloseOutput };
  "project.connections": { input: ProjectConnections; output: ProjectConnectionsOutput };
  "project.discard_recovery": { input: ProjectDiscardRecovery; output: ProjectRecoverOutput };
  "project.new": { input: ProjectNew; output: ProjectNewOutput };
  "project.open": { input: ProjectOpen; output: ProjectOpenOutput };
  "project.recover": { input: ProjectRecover; output: ProjectRecoverOutput };
  "project.rename": { input: ProjectRename; output: ProjectRenameOutput };
  "project.report_focus": { input: ProjectReportFocus; output: ProjectReportFocusOutput };
  "project.save": { input: ProjectSave; output: ProjectSaveOutput };
  "project.save_as": { input: ProjectSaveAs; output: ProjectSaveOutput };
  "project.set_allowed_roots": { input: ProjectSetAllowedRoots; output: ProjectSetAllowedRootsOutput };
  "project.set_autosave": { input: ProjectSetAutosave; output: ProjectSetAutosaveOutput };
  "timeline.add_marker": { input: TimelineAddMarker; output: TimelineAddMarkerOutput };
  "timeline.clear_in_out": { input: TimelineClearInOut; output: TimelineClearInOutOutput };
  "timeline.insert": { input: TimelineInsert; output: TimelineInsertOutput };
  "timeline.move_marker": { input: TimelineMoveMarker; output: TimelineMoveMarkerOutput };
  "timeline.remove_marker": { input: TimelineRemoveMarker; output: TimelineRemoveMarkerOutput };
  "timeline.set_in_out": { input: TimelineSetInOut; output: TimelineSetInOutOutput };
  "track.add": { input: TrackAdd; output: TrackAddOutput };
  "track.remove": { input: TrackRemove; output: TrackRemoveOutput };
  "track.reorder": { input: TrackReorder; output: TrackReorderOutput };
  "track.set": { input: TrackSet; output: TrackSetOutput };
  "track.set_height": { input: TrackSetHeight; output: TrackSetHeightOutput };
  "track.set_magnetic": { input: TrackSetMagnetic; output: TrackSetMagneticOutput };
  "view.get": { input: ViewGet; output: UiState };
  "view.select": { input: ViewSelect; output: UiState };
  "view.set": { input: ViewSet; output: UiState };
  "view.set_playhead": { input: ViewSetPlayhead; output: UiState };
  "view.set_zoom": { input: ViewSetZoom; output: UiState };
};

/** Read-only queries: name to input and output. */
export type QueryMap = {
  "item.get": { input: ItemGet; output: ItemGetOutput };
  "media.list": { input: MediaList; output: MediaListOutput };
  "project.info": { input: ProjectInfo; output: ProjectInfoOutput };
  "timeline.get": { input: TimelineGet; output: TimelineGetOutput };
  "timeline.list_markers": { input: TimelineListMarkers; output: TimelineListMarkersOutput };
  "track.list": { input: TrackList; output: TrackListOutput };
};

/** Everything callable over RPC. */
export type RpcMap = CommandMap & QueryMap;

/** Machine-readable error codes (`CmdError.code`). */
export type CmdErrorCode =
  | "not_found"
  | "locked"
  | "conflict"
  | "invalid"
  | "forbidden"
  | "unsupported"
  | "internal"
  ;
