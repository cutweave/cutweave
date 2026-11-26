// @generated — do not edit; run cargo xtask gen
import type { AudioSet, AudioSetFade, AudioSetGain, AudioSetOutput, AudioSetPan, AudioSetVolume, BeginTx, CancelTx, Checkpoint, CheckpointInfo, CheckpointList, ClipFreezeFrame, ClipFreezeFrameOutput, ClipResetSpeed, ClipReverse, ClipRippleTrim, ClipRoll, ClipSetSpeed, ClipSlide, ClipSlip, ClipSpeedOutput, ClipSplit, ClipSplitOutput, ClipTrim, ClipTrimOutput, CommitTx, DeleteCheckpoint, DeletePreset, DeletePresetOutput, DeletedOutput, ExportCancel, ExportEnqueue, ExportPauseQueue, ExportQueue, ExportQueueJob, ExportQueueOutput, ExportRemove, ExportReorder, ExportRetry, ExportStart, ExportStartOutput, GetHistory, HistoryMoved, HistoryPage, ItemApplyLayout, ItemApplyLayoutOutput, ItemAttach, ItemAttachOutput, ItemDelete, ItemDeleteOutput, ItemDetach, ItemDuplicate, ItemDuplicateOutput, ItemGet, ItemGetOutput, ItemGroupSelect, ItemGroupSelectOutput, ItemLayouts, ItemLayoutsOutput, ItemLink, ItemLinkOutput, ItemMove, ItemMoveOutput, ItemResetProps, ItemResetPropsOutput, ItemSetProps, ItemSetPropsOutput, ItemUnlink, ItemUnlinkOutput, JobCancel, JobInfo, JobList, JobListOutput, JobStatus, JobWait, KeyframeClear, KeyframeClearOutput, KeyframeCopy, KeyframeCopyOutput, KeyframeDelete, KeyframeDeleteOutput, KeyframeEval, KeyframeEvalOutput, KeyframeMove, KeyframeMoveOutput, KeyframePaste, KeyframePasteOutput, KeyframeSet, KeyframeSetInterp, KeyframeSetInterpOutput, KeyframeSetOutput, ListCheckpoints, ListLocks, ListPresets, ListPresetsOutput, LockList, MediaCacheStatus, MediaCacheStatusOutput, MediaCapabilities, MediaCapabilitiesOutput, MediaClearCache, MediaClearCacheOutput, MediaCreateFolder, MediaDeleteFolder, MediaDeleteProxy, MediaFolderCreated, MediaFolderDeleted, MediaFolderRenamed, MediaGenerateProxy, MediaImport, MediaImportOutput, MediaList, MediaListFolders, MediaListFoldersOutput, MediaListOutput, MediaMove, MediaMoved, MediaProxyStatus, MediaRegister, MediaRegisterTree, MediaRegistered, MediaRemove, MediaRemoveOutput, MediaRename, MediaRenameFolder, MediaRenamed, MediaSetCacheConfig, MediaSetDecodePolicy, MediaSetDecodePolicyOutput, MediaTreeRegistered, ProjectClose, ProjectCloseOutput, ProjectConnections, ProjectConnectionsOutput, ProjectDiscardRecovery, ProjectInfo, ProjectInfoOutput, ProjectNew, ProjectNewOutput, ProjectOpen, ProjectOpenOutput, ProjectRecover, ProjectRecoverOutput, ProjectRename, ProjectRenameOutput, ProjectReportFocus, ProjectReportFocusOutput, ProjectSave, ProjectSaveAs, ProjectSaveOutput, ProjectSetAllowedRoots, ProjectSetAllowedRootsOutput, ProjectSetAutosave, ProjectSetAutosaveOutput, ProxiesOutput, Redo, ResolvePreset, ResolvePresetOutput, Restore, Restored, SavePreset, SavePresetOutput, SequenceCreate, SequenceCreateFromMedia, SequenceCreateOutput, SequenceDelete, SequenceDeleteOutput, SequenceDuplicate, SequenceDuplicateOutput, SequenceList, SequenceListOutput, SequenceRename, SequenceRenameOutput, SequenceSetFormat, SequenceSetFormatOutput, TimelineAddMarker, TimelineAddMarkerOutput, TimelineClearInOut, TimelineClearInOutOutput, TimelineDeleteGap, TimelineDeleteGapOutput, TimelineExtract, TimelineGet, TimelineGetOutput, TimelineInsert, TimelineInsertOutput, TimelineLift, TimelineListMarkers, TimelineListMarkersOutput, TimelineMoveMarker, TimelineMoveMarkerOutput, TimelineOverwrite, TimelineRangesOutput, TimelineRemoveMarker, TimelineRemoveMarkerOutput, TimelineRippleDelete, TimelineSetInOut, TimelineSetInOutOutput, TrackAdd, TrackAddOutput, TrackList, TrackListOutput, TrackRemove, TrackRemoveOutput, TrackReorder, TrackReorderOutput, TrackSet, TrackSetHeight, TrackSetHeightOutput, TrackSetMagnetic, TrackSetMagneticOutput, TrackSetOutput, TrimOutput, TxCancelled, TxCommitted, TxOpened, UiState, Undo, ViewGet, ViewSelect, ViewSet, ViewSetActiveSequence, ViewSetActiveSequenceOutput, ViewSetPlayhead, ViewSetZoom } from "./types.ts";

/** Commands and engine commands: name to input and output. */
export type CommandMap = {
  "audio.set": { input: AudioSet; output: AudioSetOutput };
  "audio.set_fade": { input: AudioSetFade; output: AudioSetOutput };
  "audio.set_gain": { input: AudioSetGain; output: AudioSetOutput };
  "audio.set_pan": { input: AudioSetPan; output: AudioSetOutput };
  "audio.set_volume": { input: AudioSetVolume; output: AudioSetOutput };
  "clip.freeze_frame": { input: ClipFreezeFrame; output: ClipFreezeFrameOutput };
  "clip.reset_speed": { input: ClipResetSpeed; output: ClipSpeedOutput };
  "clip.reverse": { input: ClipReverse; output: ClipSpeedOutput };
  "clip.ripple_trim": { input: ClipRippleTrim; output: TrimOutput };
  "clip.roll": { input: ClipRoll; output: TrimOutput };
  "clip.set_speed": { input: ClipSetSpeed; output: ClipSpeedOutput };
  "clip.slide": { input: ClipSlide; output: TrimOutput };
  "clip.slip": { input: ClipSlip; output: TrimOutput };
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
  "export.cancel": { input: ExportCancel; output: ExportQueueJob };
  "export.delete_preset": { input: DeletePreset; output: DeletePresetOutput };
  "export.enqueue": { input: ExportEnqueue; output: ExportStartOutput };
  "export.list_presets": { input: ListPresets; output: ListPresetsOutput };
  "export.pause_queue": { input: ExportPauseQueue; output: ExportQueueOutput };
  "export.queue": { input: ExportQueue; output: ExportQueueOutput };
  "export.remove": { input: ExportRemove; output: ExportQueueOutput };
  "export.reorder": { input: ExportReorder; output: ExportQueueOutput };
  "export.resolve_preset": { input: ResolvePreset; output: ResolvePresetOutput };
  "export.retry": { input: ExportRetry; output: ExportQueueJob };
  "export.save_preset": { input: SavePreset; output: SavePresetOutput };
  "export.start": { input: ExportStart; output: ExportStartOutput };
  "item.apply_layout": { input: ItemApplyLayout; output: ItemApplyLayoutOutput };
  "item.attach": { input: ItemAttach; output: ItemAttachOutput };
  "item.delete": { input: ItemDelete; output: ItemDeleteOutput };
  "item.detach": { input: ItemDetach; output: ItemAttachOutput };
  "item.duplicate": { input: ItemDuplicate; output: ItemDuplicateOutput };
  "item.link": { input: ItemLink; output: ItemLinkOutput };
  "item.move": { input: ItemMove; output: ItemMoveOutput };
  "item.reset_props": { input: ItemResetProps; output: ItemResetPropsOutput };
  "item.set_props": { input: ItemSetProps; output: ItemSetPropsOutput };
  "item.unlink": { input: ItemUnlink; output: ItemUnlinkOutput };
  "job.cancel": { input: JobCancel; output: JobInfo };
  "job.list": { input: JobList; output: JobListOutput };
  "job.status": { input: JobStatus; output: JobInfo };
  "job.wait": { input: JobWait; output: JobInfo };
  "keyframe.clear": { input: KeyframeClear; output: KeyframeClearOutput };
  "keyframe.delete": { input: KeyframeDelete; output: KeyframeDeleteOutput };
  "keyframe.move": { input: KeyframeMove; output: KeyframeMoveOutput };
  "keyframe.paste": { input: KeyframePaste; output: KeyframePasteOutput };
  "keyframe.set": { input: KeyframeSet; output: KeyframeSetOutput };
  "keyframe.set_interp": { input: KeyframeSetInterp; output: KeyframeSetInterpOutput };
  "media.cache_status": { input: MediaCacheStatus; output: MediaCacheStatusOutput };
  "media.capabilities": { input: MediaCapabilities; output: MediaCapabilitiesOutput };
  "media.clear_cache": { input: MediaClearCache; output: MediaClearCacheOutput };
  "media.create_folder": { input: MediaCreateFolder; output: MediaFolderCreated };
  "media.delete_folder": { input: MediaDeleteFolder; output: MediaFolderDeleted };
  "media.delete_proxy": { input: MediaDeleteProxy; output: ProxiesOutput };
  "media.generate_proxy": { input: MediaGenerateProxy; output: ProxiesOutput };
  "media.import": { input: MediaImport; output: MediaImportOutput };
  "media.move": { input: MediaMove; output: MediaMoved };
  "media.proxy_status": { input: MediaProxyStatus; output: ProxiesOutput };
  "media.register": { input: MediaRegister; output: MediaRegistered };
  "media.register_tree": { input: MediaRegisterTree; output: MediaTreeRegistered };
  "media.remove": { input: MediaRemove; output: MediaRemoveOutput };
  "media.rename": { input: MediaRename; output: MediaRenamed };
  "media.rename_folder": { input: MediaRenameFolder; output: MediaFolderRenamed };
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
  "sequence.create": { input: SequenceCreate; output: SequenceCreateOutput };
  "sequence.create_from_media": { input: SequenceCreateFromMedia; output: SequenceCreateOutput };
  "sequence.delete": { input: SequenceDelete; output: SequenceDeleteOutput };
  "sequence.duplicate": { input: SequenceDuplicate; output: SequenceDuplicateOutput };
  "sequence.rename": { input: SequenceRename; output: SequenceRenameOutput };
  "sequence.set_format": { input: SequenceSetFormat; output: SequenceSetFormatOutput };
  "timeline.add_marker": { input: TimelineAddMarker; output: TimelineAddMarkerOutput };
  "timeline.clear_in_out": { input: TimelineClearInOut; output: TimelineClearInOutOutput };
  "timeline.delete_gap": { input: TimelineDeleteGap; output: TimelineDeleteGapOutput };
  "timeline.extract": { input: TimelineExtract; output: TimelineRangesOutput };
  "timeline.insert": { input: TimelineInsert; output: TimelineInsertOutput };
  "timeline.lift": { input: TimelineLift; output: TimelineRangesOutput };
  "timeline.move_marker": { input: TimelineMoveMarker; output: TimelineMoveMarkerOutput };
  "timeline.overwrite": { input: TimelineOverwrite; output: TimelineInsertOutput };
  "timeline.remove_marker": { input: TimelineRemoveMarker; output: TimelineRemoveMarkerOutput };
  "timeline.ripple_delete": { input: TimelineRippleDelete; output: DeletedOutput };
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
  "view.set_active_sequence": { input: ViewSetActiveSequence; output: ViewSetActiveSequenceOutput };
  "view.set_playhead": { input: ViewSetPlayhead; output: UiState };
  "view.set_zoom": { input: ViewSetZoom; output: UiState };
};

/** Read-only queries: name to input and output. */
export type QueryMap = {
  "item.get": { input: ItemGet; output: ItemGetOutput };
  "item.group_select": { input: ItemGroupSelect; output: ItemGroupSelectOutput };
  "item.layouts": { input: ItemLayouts; output: ItemLayoutsOutput };
  "keyframe.copy": { input: KeyframeCopy; output: KeyframeCopyOutput };
  "keyframe.eval": { input: KeyframeEval; output: KeyframeEvalOutput };
  "media.list": { input: MediaList; output: MediaListOutput };
  "media.list_folders": { input: MediaListFolders; output: MediaListFoldersOutput };
  "project.info": { input: ProjectInfo; output: ProjectInfoOutput };
  "sequence.list": { input: SequenceList; output: SequenceListOutput };
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
