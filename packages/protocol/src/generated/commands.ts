// @generated — do not edit; run cargo xtask gen
import type { BeginTx, CancelTx, Checkpoint, CheckpointInfo, CheckpointList, ClipSplit, ClipSplitOutput, ClipTrim, ClipTrimOutput, CommitTx, GetHistory, HistoryMoved, HistoryPage, ItemDelete, ItemDeleteOutput, ItemGet, ItemGetOutput, ItemMove, ItemMoveOutput, ItemSetProps, ItemSetPropsOutput, JobCancel, JobInfo, JobList, JobListOutput, JobStatus, JobWait, ListCheckpoints, ListLocks, LockList, MediaImport, MediaImportOutput, MediaList, MediaListOutput, MediaRegister, MediaRegistered, MediaRemove, MediaRemoveOutput, ProjectInfo, ProjectInfoOutput, ProjectNew, ProjectNewOutput, ProjectRename, ProjectRenameOutput, Redo, Restore, Restored, TimelineGet, TimelineGetOutput, TimelineInsert, TimelineInsertOutput, TrackAdd, TrackAddOutput, TrackRemove, TrackRemoveOutput, TrackSet, TrackSetOutput, TxCancelled, TxCommitted, TxOpened, UiState, Undo, ViewGet, ViewSelect, ViewSet, ViewSetPlayhead, ViewSetZoom } from "./types.ts";

/** Commands and engine commands: name to input and output. */
export type CommandMap = {
  "clip.split": { input: ClipSplit; output: ClipSplitOutput };
  "clip.trim": { input: ClipTrim; output: ClipTrimOutput };
  "edit.begin_tx": { input: BeginTx; output: TxOpened };
  "edit.cancel_tx": { input: CancelTx; output: TxCancelled };
  "edit.checkpoint": { input: Checkpoint; output: CheckpointInfo };
  "edit.commit_tx": { input: CommitTx; output: TxCommitted };
  "edit.history": { input: GetHistory; output: HistoryPage };
  "edit.list_checkpoints": { input: ListCheckpoints; output: CheckpointList };
  "edit.locks": { input: ListLocks; output: LockList };
  "edit.redo": { input: Redo; output: HistoryMoved };
  "edit.restore": { input: Restore; output: Restored };
  "edit.undo": { input: Undo; output: HistoryMoved };
  "item.delete": { input: ItemDelete; output: ItemDeleteOutput };
  "item.move": { input: ItemMove; output: ItemMoveOutput };
  "item.set_props": { input: ItemSetProps; output: ItemSetPropsOutput };
  "job.cancel": { input: JobCancel; output: JobInfo };
  "job.list": { input: JobList; output: JobListOutput };
  "job.status": { input: JobStatus; output: JobInfo };
  "job.wait": { input: JobWait; output: JobInfo };
  "media.import": { input: MediaImport; output: MediaImportOutput };
  "media.register": { input: MediaRegister; output: MediaRegistered };
  "media.remove": { input: MediaRemove; output: MediaRemoveOutput };
  "project.new": { input: ProjectNew; output: ProjectNewOutput };
  "project.rename": { input: ProjectRename; output: ProjectRenameOutput };
  "timeline.insert": { input: TimelineInsert; output: TimelineInsertOutput };
  "track.add": { input: TrackAdd; output: TrackAddOutput };
  "track.remove": { input: TrackRemove; output: TrackRemoveOutput };
  "track.set": { input: TrackSet; output: TrackSetOutput };
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
