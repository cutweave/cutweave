// SPDX-License-Identifier: GPL-3.0-or-later
// The Preview's menus and the transport's (WP-2H.13): the picture, the Source view, the timecode,
// the volume and the quality badge.
// TODO(render.frame): "Copy frame" and "Save frame as…" need `render.frame` in the command
// catalog and an image clipboard / save dialog in the desktop host; neither exists yet.
// The grid, center cross and zoom choices are the preview bar's Guides menu (WP-2H.06).
// The Source view's Insert, Overwrite and marks are WP-2A.14: the marks are view state and the
// placements are `timeline.insert` and `timeline.overwrite` (source/actions.ts).
import { PLAYING_QUALITIES, type PlayingQualityChoice } from "@cutweave/preferences";
import type { TKey } from "../../i18n/index.ts";
import { prefsStore } from "../../prefs/prefsStore.ts";
import { sourceHasMarks } from "../../source/actions.ts";
import { flipArgs, placeArgs } from "../../preview/overlays/placement.ts";
import { docStore } from "../../state/docStore.ts";
import { playbackStore } from "../../state/playbackStore.ts";
import { viewStore } from "../../state/viewStore.ts";
import { displayTime } from "../../timeline/prefs.ts";
import type { LabelSpec, MenuContext, MenuRow, MenuSurfaceDef } from "../types.ts";
import { clipFact } from "./facts.ts";
import { sequenceRate } from "./shared.ts";

const NO_ENGINE = "contextMenu.reason.noEngine" as const;

const playLabel = (): LabelSpec => ({
  key: playbackStore.getState().playing ? "transport.pause" : "transport.play",
});

const playRow = (group: string, order: number): MenuRow => ({
  command: "transport.toggle_play",
  group,
  order,
  label: playLabel,
  when: "engine.live",
  reason: "contextMenu.reason.noEngine",
});

const QUALITY_LABELS: Readonly<Record<PlayingQualityChoice, TKey>> = {
  auto: "transport.quality.auto",
  full: "transport.quality.full",
  half: "transport.quality.half",
  quarter: "transport.quality.quarter",
};

/**
 * One choice of the preview's quality while playing: the preference `playback.quality_playing`
 * (WP-2H.10), the same value as its row in Settings > Playback.
 */
const qualityRow = (quality: PlayingQualityChoice, order: number, group = "quality"): MenuRow => ({
  id: `quality-${quality}`,
  command: "view.set_playing_quality",
  group,
  order,
  radio: true,
  label: { key: QUALITY_LABELS[quality] },
  checked: () => prefsStore.getState().values["playback.quality_playing"] === quality,
  args: () => ({ quality }),
});

const qualityRows = (group?: string): MenuRow[] =>
  PLAYING_QUALITIES.map((quality, order) => qualityRow(quality, order, group));

/** The timecode the transport shows now: the source's own while one plays, else the timeline's. */
export function shownTimecode(): string {
  const pb = playbackStore.getState();
  const values = prefsStore.getState().values;
  if (pb.source !== null) return displayTime(pb.sourcePlayhead, pb.source.fps, values);
  const rate = sequenceRate() ?? { num: 30, den: 1 };
  return displayTime(viewStore.getState().playhead, rate, values);
}

/** "Add frame to media bin" (WP-2F.05d): the frame on screen becomes a picture in the bin. */
const addFrameRow = (group: string, order: number): MenuRow => ({
  command: "preview.add_frame_to_bin",
  group,
  order,
  when: "engine.live",
  reason: "contextMenu.reason.noEngine",
});

export const PREVIEW_CANVAS: MenuSurfaceDef = {
  id: "preview.canvas",
  name: () => ({ key: "contextMenu.name.previewCanvas" }),
  groups: ["play", "navigate", "frame", "view", "quality"],
  rows: [
    addFrameRow("frame", 0),
    playRow("play", 0),
    {
      command: "transport.to_start",
      group: "navigate",
      order: 0,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
    },
    {
      command: "transport.to_end",
      group: "navigate",
      order: 1,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
    },
    {
      command: "view.toggle_safe_areas",
      group: "view",
      order: 0,
      checked: () => viewStore.getState().safeAreas,
    },
    {
      id: "quality",
      group: "quality",
      order: 0,
      submenu: { label: { key: "contextMenu.item.quality" }, rows: qualityRows("quality") },
    },
  ],
};

export const SOURCE_VIEW: MenuSurfaceDef = {
  id: "source.view",
  name: () => ({ key: "contextMenu.name.sourceView" }),
  groups: ["play", "place", "marks", "navigate", "frame", "copy"],
  rows: [
    playRow("play", 0),
    { command: "source.insert", group: "place", order: 0, when: "engine.live", reason: NO_ENGINE },
    {
      command: "source.overwrite",
      group: "place",
      order: 1,
      when: "engine.live",
      reason: NO_ENGINE,
    },
    { command: "source.mark_in", group: "marks", order: 0 },
    { command: "source.mark_out", group: "marks", order: 1 },
    {
      command: "source.clear_marks",
      group: "marks",
      order: 2,
      enabledIf: () => sourceHasMarks(),
      reason: "contextMenu.reason.noMarks",
    },
    { command: "source.back", group: "navigate", order: 0 },
    addFrameRow("frame", 0),
    {
      id: "copy-timecode",
      command: "host.copy_text",
      group: "copy",
      order: 0,
      label: { key: "contextMenu.item.copyTimecode" },
      args: () => ({ text: shownTimecode() }),
    },
  ],
};

export const TRANSPORT_TIME: MenuSurfaceDef = {
  id: "transport.time",
  name: () => ({ key: "contextMenu.name.transportTime" }),
  groups: ["copy"],
  rows: [
    {
      id: "copy-timecode",
      command: "host.copy_text",
      group: "copy",
      order: 0,
      label: { key: "contextMenu.item.copyTimecode" },
      args: () => ({ text: shownTimecode() }),
    },
  ],
};

const volumeIsUnity = (_ctx: MenuContext): boolean => viewStore.getState().masterGain !== 1;

export const TRANSPORT_VOLUME: MenuSurfaceDef = {
  id: "transport.volume",
  name: () => ({ key: "contextMenu.name.transportVolume" }),
  groups: ["volume"],
  rows: [
    {
      command: "transport.toggle_mute",
      group: "volume",
      order: 0,
      label: () => ({ key: viewStore.getState().muted ? "transport.unmute" : "transport.mute" }),
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
    },
    {
      command: "transport.reset_volume",
      group: "volume",
      order: 1,
      when: "engine.live",
      enabledIf: volumeIsUnity,
      reason: "contextMenu.reason.alreadyDefault",
    },
  ],
};

export const TRANSPORT_QUALITY: MenuSurfaceDef = {
  id: "transport.quality",
  name: () => ({ key: "contextMenu.name.transportQuality" }),
  groups: ["quality"],
  rows: qualityRows("quality"),
};

// ---- the picture of a selected clip (WP-2H.06) ---------------------------------------------------

const LOCKED = "contextMenu.reason.trackLocked" as const;

/** The playhead the menu acts on: where the picture on screen is. */
const playheadOf = (ctx: MenuContext): number => ctx.playhead ?? viewStore.getState().playhead;

const unlocked = (ctx: MenuContext): boolean => ctx.keys["track.locked"] !== true;
const oneAndUnlocked = (ctx: MenuContext): boolean => ctx.ids.length === 1 && unlocked(ctx);
const oneReason = (ctx: MenuContext) =>
  ctx.keys["track.locked"] === true ? LOCKED : ("contextMenu.reason.oneClip" as const);

const placeRow = (
  id: string,
  mode: "fit" | "fill" | "center",
  label:
    | "contextMenu.item.fitCanvas"
    | "contextMenu.item.fillCanvas"
    | "contextMenu.item.centerCanvas",
  order: number,
): MenuRow => ({
  id,
  command: "item.set_props",
  group: "place",
  order,
  label: { key: label },
  enabledIf: oneAndUnlocked,
  reason: oneReason,
  args: (ctx) => placeArgs(docStore.getState().project, ctx.ids[0] ?? "", playheadOf(ctx), mode),
});

const flipRow = (axis: "horizontal" | "vertical", order: number): MenuRow => ({
  id: `flip-${axis}`,
  command: "item.set_props",
  group: "flip",
  order,
  label: {
    key:
      axis === "horizontal" ? "contextMenu.item.flipHorizontal" : "contextMenu.item.flipVertical",
  },
  enabledIf: (ctx) => ctx.ids.length > 0 && unlocked(ctx),
  reason: LOCKED,
  args: (ctx) => flipArgs(docStore.getState().project, ctx.ids, playheadOf(ctx), axis),
});

/** Right-click on the selected clip's picture: where it sits, its mirror and its reset. */
export const PREVIEW_ITEM: MenuSurfaceDef = {
  id: "preview.item",
  name: (target) => ({
    key: "contextMenu.name.previewItem",
    vars: { name: target.name ?? "" },
  }),
  groups: ["place", "flip", "reset"],
  keys: (target) => ({
    "track.locked": target.ids.some((id) => clipFact(id)?.track.locked === true),
  }),
  rows: [
    placeRow("fit", "fit", "contextMenu.item.fitCanvas", 0),
    placeRow("fill", "fill", "contextMenu.item.fillCanvas", 1),
    placeRow("center", "center", "contextMenu.item.centerCanvas", 2),
    flipRow("horizontal", 0),
    flipRow("vertical", 1),
    {
      id: "reset-transform",
      command: "item.reset_props",
      group: "reset",
      order: 0,
      label: { key: "contextMenu.item.resetTransform" },
      enabledIf: (ctx) => ctx.ids.length > 0 && unlocked(ctx),
      reason: LOCKED,
      args: (ctx) => ({ items: ctx.ids, group: "transform" }),
    },
  ],
};
