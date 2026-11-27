// SPDX-License-Identifier: GPL-3.0-or-later
// The commands only a key reaches today, and the ones the palette lists that a key also runs
// (WP-2H.05). Each has a stable id, the group Settings > Keyboard shortcuts lists it under, the
// context where its key works and its words. Engine commands (`clip.split`) and UI commands
// (`project.save`, `workspace.toggle_panel.timeline`) keep their own ids; the keys that had no
// command of their own (the transport's, the bin's) get one here, so the palette and the shortcut
// editor list the same things.
import type { TKey } from "../i18n/index.ts";

/**
 * Where a key works. `global` is everywhere (text fields keep their plain keys); `playback` is the
 * transport's keys, everywhere except where the bin has its own use of the same key; `timeline`,
 * `bin`, `source` and `preview` (the picture's handles) work while that panel has the focus.
 */
export type KeyContext = "global" | "playback" | "timeline" | "bin" | "source" | "preview";

export type BindingGroupId = "menu" | "playback" | "timeline" | "preview" | "bin" | "dialogs";

export interface KeyCommand {
  /** The command id: `edit.undo`, `transport.toggle_play`. */
  readonly id: string;
  readonly group: BindingGroupId;
  readonly context: KeyContext;
  readonly label: TKey;
  readonly description: TKey;
  /** The keys cannot be changed (the text fields' and dialogs' own). */
  readonly fixed?: boolean;
  /** Only in an unpackaged (development) build. */
  readonly devOnly?: boolean;
}

const cmd = (
  id: string,
  group: BindingGroupId,
  context: KeyContext,
  label: TKey,
  description: TKey,
  flags: { fixed?: true; devOnly?: true } = {},
): KeyCommand => ({ id, group, context, label, description, ...flags });

/** Every command a key can run that is not an engine command, in the order the page lists them. */
export const KEY_COMMANDS: readonly KeyCommand[] = [
  // ---- the menu bar
  cmd("project.new", "menu", "global", "menu.new", "shortcuts.menu.new"),
  cmd("project.open", "menu", "global", "menu.open", "shortcuts.menu.open"),
  cmd("project.save", "menu", "global", "menu.save", "shortcuts.menu.save"),
  cmd("project.save_as", "menu", "global", "menu.saveAs", "shortcuts.menu.saveAs"),
  cmd("project.collect", "menu", "global", "menu.collect", "shortcuts.menu.collect"),
  cmd("project.archive", "menu", "global", "menu.archive", "shortcuts.menu.archive"),
  cmd(
    "project.set_settings",
    "menu",
    "global",
    "menu.projectSettings",
    "shortcuts.menu.projectSettings",
  ),
  cmd("export.start", "menu", "global", "menu.export", "shortcuts.menu.export"),
  cmd("app.open_settings", "menu", "global", "menu.settings", "shortcuts.menu.settings"),
  cmd("app.quit", "menu", "global", "menu.quit", "shortcuts.menu.quit"),
  cmd("edit.undo", "menu", "global", "shortcuts.menu.undoLabel", "shortcuts.menu.undo"),
  cmd("edit.redo", "menu", "global", "shortcuts.menu.redoLabel", "shortcuts.menu.redo"),
  cmd("edit.text.cut", "menu", "global", "menu.cut", "shortcuts.menu.cut", { fixed: true }),
  cmd("edit.text.copy", "menu", "global", "menu.copy", "shortcuts.menu.copy", { fixed: true }),
  cmd("edit.text.paste", "menu", "global", "menu.paste", "shortcuts.menu.paste", { fixed: true }),
  cmd("edit.text.select_all", "menu", "global", "menu.selectAll", "shortcuts.menu.selectAll", {
    fixed: true,
  }),
  cmd(
    "window.toggle_full_screen",
    "menu",
    "global",
    "menu.toggleFullScreen",
    "shortcuts.menu.fullScreen",
  ),
  cmd("window.reload", "menu", "global", "menu.reload", "shortcuts.menu.reload", { devOnly: true }),
  cmd(
    "window.toggle_dev_tools",
    "menu",
    "global",
    "menu.toggleDevTools",
    "shortcuts.menu.devTools",
    { devOnly: true },
  ),
  cmd("palette.open", "menu", "global", "palette.title", "shortcuts.menu.commandPalette"),
  // ---- playback
  cmd(
    "transport.toggle_play",
    "playback",
    "playback",
    "shortcuts.playback.togglePlay",
    "shortcuts.playback.togglePlayHelp",
  ),
  cmd(
    "transport.reverse",
    "playback",
    "playback",
    "shortcuts.playback.reverse",
    "shortcuts.playback.reverseHelp",
  ),
  cmd(
    "transport.pause",
    "playback",
    "playback",
    "shortcuts.playback.pause",
    "shortcuts.playback.pauseHelp",
  ),
  cmd(
    "transport.forward",
    "playback",
    "playback",
    "shortcuts.playback.forward",
    "shortcuts.playback.forwardHelp",
  ),
  cmd(
    "transport.step_back",
    "playback",
    "playback",
    "shortcuts.playback.stepBack",
    "shortcuts.playback.stepBackHelp",
  ),
  cmd(
    "transport.step_forward",
    "playback",
    "playback",
    "shortcuts.playback.stepForward",
    "shortcuts.playback.stepForwardHelp",
  ),
  cmd("source.back", "playback", "source", "shortcuts.source.back", "shortcuts.source.backHelp"),
  cmd(
    "source.insert",
    "playback",
    "source",
    "shortcuts.source.insert",
    "shortcuts.source.insertHelp",
  ),
  cmd(
    "source.overwrite",
    "playback",
    "source",
    "shortcuts.source.overwrite",
    "shortcuts.source.overwriteHelp",
  ),
  cmd(
    "source.mark_in",
    "playback",
    "source",
    "shortcuts.source.markIn",
    "shortcuts.source.markInHelp",
  ),
  cmd(
    "source.mark_out",
    "playback",
    "source",
    "shortcuts.source.markOut",
    "shortcuts.source.markOutHelp",
  ),
  cmd(
    "source.clear_marks",
    "playback",
    "source",
    "shortcuts.source.clearMarks",
    "shortcuts.source.clearMarksHelp",
  ),
  // ---- the timeline
  cmd(
    "timeline.cancel",
    "timeline",
    "timeline",
    "shortcuts.timeline.cancel",
    "shortcuts.timeline.cancelHelp",
  ),
  cmd(
    "clip.split",
    "timeline",
    "timeline",
    "shortcuts.timeline.split",
    "shortcuts.timeline.splitHelp",
  ),
  cmd(
    "item.delete",
    "timeline",
    "timeline",
    "shortcuts.timeline.delete",
    "shortcuts.timeline.deleteHelp",
  ),
  cmd(
    "item.duplicate",
    "timeline",
    "timeline",
    "shortcuts.timeline.duplicate",
    "shortcuts.timeline.duplicateHelp",
  ),
  cmd(
    "timeline.toggle_snapping",
    "timeline",
    "timeline",
    "shortcuts.timeline.snapping",
    "shortcuts.timeline.snappingHelp",
  ),
  cmd(
    "track.rename",
    "timeline",
    "timeline",
    "shortcuts.timeline.renameTrack",
    "shortcuts.timeline.renameTrackHelp",
  ),
  cmd(
    "timeline.ripple_delete",
    "timeline",
    "timeline",
    "shortcuts.timeline.rippleDelete",
    "shortcuts.timeline.rippleDeleteHelp",
  ),
  cmd(
    "timeline.nudge_left",
    "timeline",
    "timeline",
    "shortcuts.timeline.nudgeLeft",
    "shortcuts.timeline.nudgeLeftHelp",
  ),
  cmd(
    "timeline.nudge_right",
    "timeline",
    "timeline",
    "shortcuts.timeline.nudgeRight",
    "shortcuts.timeline.nudgeRightHelp",
  ),
  cmd(
    "timeline.nudge_left_10",
    "timeline",
    "timeline",
    "shortcuts.timeline.nudgeLeftFar",
    "shortcuts.timeline.nudgeLeftFarHelp",
  ),
  cmd(
    "timeline.nudge_right_10",
    "timeline",
    "timeline",
    "shortcuts.timeline.nudgeRightFar",
    "shortcuts.timeline.nudgeRightFarHelp",
  ),
  cmd(
    "timeline.select_all",
    "timeline",
    "timeline",
    "shortcuts.timeline.selectAll",
    "shortcuts.timeline.selectAllHelp",
  ),
  cmd(
    "timeline.deselect",
    "timeline",
    "timeline",
    "shortcuts.timeline.deselect",
    "shortcuts.timeline.deselectHelp",
  ),
  cmd(
    "timeline.add_marker",
    "timeline",
    "timeline",
    "shortcuts.timeline.addMarker",
    "shortcuts.timeline.addMarkerHelp",
  ),
  cmd(
    "timeline.mark_in",
    "timeline",
    "timeline",
    "shortcuts.timeline.markIn",
    "shortcuts.timeline.markInHelp",
  ),
  cmd(
    "timeline.mark_out",
    "timeline",
    "timeline",
    "shortcuts.timeline.markOut",
    "shortcuts.timeline.markOutHelp",
  ),
  cmd(
    "timeline.match_frame",
    "timeline",
    "timeline",
    "shortcuts.timeline.matchFrame",
    "shortcuts.timeline.matchFrameHelp",
  ),
  cmd(
    "timeline.tool.select",
    "timeline",
    "timeline",
    "shortcuts.timeline.toolSelect",
    "shortcuts.timeline.toolSelectHelp",
  ),
  cmd(
    "timeline.tool.ripple",
    "timeline",
    "timeline",
    "shortcuts.timeline.toolRipple",
    "shortcuts.timeline.toolRippleHelp",
  ),
  cmd(
    "timeline.tool.roll",
    "timeline",
    "timeline",
    "shortcuts.timeline.toolRoll",
    "shortcuts.timeline.toolRollHelp",
  ),
  cmd(
    "timeline.tool.slip",
    "timeline",
    "timeline",
    "shortcuts.timeline.toolSlip",
    "shortcuts.timeline.toolSlipHelp",
  ),
  cmd(
    "timeline.tool.slide",
    "timeline",
    "timeline",
    "shortcuts.timeline.toolSlide",
    "shortcuts.timeline.toolSlideHelp",
  ),
  cmd(
    "timeline.tool.razor",
    "timeline",
    "timeline",
    "shortcuts.timeline.toolRazor",
    "shortcuts.timeline.toolRazorHelp",
  ),
  cmd(
    "timeline.tool.hand",
    "timeline",
    "timeline",
    "shortcuts.timeline.toolHand",
    "shortcuts.timeline.toolHandHelp",
  ),
  cmd(
    "timeline.tool.zoom",
    "timeline",
    "timeline",
    "shortcuts.timeline.toolZoom",
    "shortcuts.timeline.toolZoomHelp",
  ),
  // ---- the preview's handles
  cmd(
    "preview.nudge_left",
    "preview",
    "preview",
    "shortcuts.preview.nudgeLeft",
    "shortcuts.preview.nudgeHelp",
  ),
  cmd(
    "preview.nudge_right",
    "preview",
    "preview",
    "shortcuts.preview.nudgeRight",
    "shortcuts.preview.nudgeHelp",
  ),
  cmd(
    "preview.nudge_up",
    "preview",
    "preview",
    "shortcuts.preview.nudgeUp",
    "shortcuts.preview.nudgeHelp",
  ),
  cmd(
    "preview.nudge_down",
    "preview",
    "preview",
    "shortcuts.preview.nudgeDown",
    "shortcuts.preview.nudgeHelp",
  ),
  cmd(
    "preview.toggle_crop",
    "preview",
    "preview",
    "shortcuts.preview.toggleCrop",
    "shortcuts.preview.toggleCropHelp",
  ),
  // ---- the media bin
  cmd("bin.move_next", "bin", "bin", "shortcuts.bin.moveNext", "shortcuts.bin.moveHelp"),
  cmd("bin.move_previous", "bin", "bin", "shortcuts.bin.movePrevious", "shortcuts.bin.moveHelp"),
  cmd("bin.move_down", "bin", "bin", "shortcuts.bin.moveDown", "shortcuts.bin.moveHelp"),
  cmd("bin.move_up", "bin", "bin", "shortcuts.bin.moveUp", "shortcuts.bin.moveHelp"),
  cmd("bin.first", "bin", "bin", "shortcuts.bin.first", "shortcuts.bin.edgesHelp"),
  cmd("bin.last", "bin", "bin", "shortcuts.bin.last", "shortcuts.bin.edgesHelp"),
  cmd("bin.insert", "bin", "bin", "shortcuts.bin.insert", "shortcuts.bin.insertHelp"),
  cmd("bin.play", "bin", "bin", "shortcuts.bin.play", "shortcuts.bin.playHelp"),
  cmd("media.remove", "bin", "bin", "shortcuts.bin.remove", "shortcuts.bin.removeHelp"),
  cmd("bin.import", "bin", "global", "shortcuts.bin.import", "shortcuts.bin.importHelp"),
  cmd(
    "bin.import_folder",
    "bin",
    "global",
    "shortcuts.bin.importFolder",
    "shortcuts.bin.importFolderHelp",
  ),
  cmd("bin.search", "bin", "bin", "shortcuts.bin.search", "shortcuts.bin.searchHelp"),
  cmd("bin.up", "bin", "bin", "shortcuts.bin.up", "shortcuts.bin.upHelp"),
  cmd("bin.rename", "bin", "bin", "shortcuts.bin.rename", "shortcuts.bin.renameHelp"),
  cmd("bin.new_folder", "bin", "bin", "shortcuts.bin.newFolder", "shortcuts.bin.newFolderHelp"),
  // ---- dialogs and text fields: the UI library owns these keys
  cmd(
    "dialogs.close",
    "dialogs",
    "global",
    "shortcuts.dialogs.close",
    "shortcuts.dialogs.closeHelp",
    { fixed: true },
  ),
  cmd(
    "dialogs.confirm",
    "dialogs",
    "global",
    "shortcuts.dialogs.confirm",
    "shortcuts.dialogs.confirmHelp",
    { fixed: true },
  ),
  cmd("dialogs.next", "dialogs", "global", "shortcuts.dialogs.next", "shortcuts.dialogs.nextHelp", {
    fixed: true,
  }),
];

const BY_ID = new Map(KEY_COMMANDS.map((c) => [c.id, c]));

/** The key command with this id, if it is one of the table's. */
export function keyCommand(id: string): KeyCommand | undefined {
  return BY_ID.get(id);
}

/** Two contexts can both be live for one key press: a key bound in both would clash. */
export function contextsOverlap(a: KeyContext, b: KeyContext): boolean {
  if (a === b || a === "global" || b === "global") return true;
  // The bin's own keys (Space, the arrows) take precedence where it has the focus; that is by
  // design, so a playback key and a bin (or preview) key may share a stroke. Everywhere else playback is live.
  if (a === "playback") return b !== "bin" && b !== "preview";
  if (b === "playback") return a !== "bin" && a !== "preview";
  return false;
}
