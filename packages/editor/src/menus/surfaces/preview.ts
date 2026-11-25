// SPDX-License-Identifier: GPL-3.0-or-later
// The Preview's menus and the transport's (WP-2H.13): the picture, the Source view, the timecode,
// the volume and the quality badge.
// TODO(render.frame): "Copy frame" and "Save frame as…" need `render.frame` in the command
// catalog and an image clipboard / save dialog in the desktop host; neither exists yet.
// TODO(WP-2H.06): grid, center cross and zoom choices. TODO(WP-2A.14): the Source view's
// Insert, Overwrite and marks.
import type { PreviewQuality } from "@cutweave/protocol";
import { formatTimecode } from "@cutweave/timeline";
import { playbackStore } from "../../state/playbackStore.ts";
import { viewStore } from "../../state/viewStore.ts";
import type { LabelSpec, MenuContext, MenuRow, MenuSurfaceDef } from "../types.ts";
import { sequenceRate } from "./shared.ts";

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

const QUALITIES: readonly PreviewQuality[] = ["full", "half", "quarter"];

const qualityRow = (quality: PreviewQuality, order: number, group = "quality"): MenuRow => ({
  id: `quality-${quality}`,
  command: "view.set_preview_quality",
  group,
  order,
  radio: true,
  label: {
    key:
      quality === "full"
        ? "transport.quality.full"
        : quality === "half"
          ? "transport.quality.half"
          : "transport.quality.quarter",
  },
  checked: () => viewStore.getState().previewQuality === quality,
  args: () => ({ quality }),
});

const qualityRows = (group?: string): MenuRow[] =>
  QUALITIES.map((quality, order) => qualityRow(quality, order, group));

/** The timecode the transport shows now: the source's own while one plays, else the timeline's. */
export function shownTimecode(): string {
  const pb = playbackStore.getState();
  if (pb.source !== null) return formatTimecode(pb.sourcePlayhead, pb.source.fps, true);
  const rate = sequenceRate() ?? { num: 30, den: 1 };
  return formatTimecode(viewStore.getState().playhead, rate, true);
}

export const PREVIEW_CANVAS: MenuSurfaceDef = {
  id: "preview.canvas",
  name: () => ({ key: "contextMenu.name.previewCanvas" }),
  groups: ["play", "navigate", "view", "quality"],
  rows: [
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
  groups: ["play", "navigate", "copy"],
  rows: [
    playRow("play", 0),
    { command: "source.back", group: "navigate", order: 0 },
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
