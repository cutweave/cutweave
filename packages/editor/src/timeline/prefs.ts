// SPDX-License-Identifier: GPL-3.0-or-later
// What the Editing and Timeline pages of Settings mean to the timeline (WP-2A.13): the machine's
// environment (snap distance and kinds, what a drop onto a clip does, the nudge step), how the
// canvas shows things (time, thumbnails, waveforms, the wheel), the height a new track gets and
// whether a placed video brings its sound. Pure functions of the preference values, so a change
// in Settings reaches the open timeline at once and nothing is written to the project except
// through the commands these values feed.
import type { PreferenceMap, SnapTarget, TrackHeightChoice } from "@cutweave/preferences";
import {
  type FrameRate,
  flicksToFrame,
  formatSeconds,
  formatTimecode,
  type MachineEnv,
  type SnapKindName,
  type TimelineDisplay,
} from "@cutweave/timeline";

type Values = Pick<
  PreferenceMap,
  | "editing.drop_mode"
  | "editing.nudge"
  | "timeline.snap_targets"
  | "timeline.snap_distance"
  | "timeline.thumbnails"
  | "timeline.waveforms"
  | "timeline.wheel"
  | "timeline.time_display"
  | "timeline.drop_frame"
>;

/** The machine's part of the environment that comes from Settings. */
export type MachinePrefs = Pick<
  MachineEnv,
  "snapThresholdPx" | "snapKinds" | "dropOnClip" | "nudgeFrames"
>;

/** The engine's snap kinds each "Snap to" switch stands for (in and out go with the markers). */
const KINDS: Readonly<Record<SnapTarget, readonly SnapKindName[]>> = {
  playhead: ["playhead"],
  edges: ["edge"],
  markers: ["marker", "in_out"],
  beats: ["beat"],
};

export function snapKindsOf(targets: readonly SnapTarget[]): SnapKindName[] {
  return targets.flatMap((target) => KINDS[target]);
}

export function machinePrefs(values: Values): MachinePrefs {
  return {
    snapThresholdPx: values["timeline.snap_distance"],
    snapKinds: snapKindsOf(values["timeline.snap_targets"]),
    dropOnClip: values["editing.drop_mode"],
    nudgeFrames: values["editing.nudge"],
  };
}

/** The canvas's `display` prop. */
export function timelineDisplay(values: Values): TimelineDisplay {
  return {
    timeFormat: values["timeline.time_display"],
    dropFrame: values["timeline.drop_frame"],
    thumbnails: values["timeline.thumbnails"],
    waveforms: values["timeline.waveforms"],
    wheel: values["timeline.wheel"],
  };
}

/** How a time shows outside the canvas (the transport, "Copy value"). */
export function displayTime(
  t: number,
  fps: FrameRate,
  values: Pick<PreferenceMap, "timeline.time_display" | "timeline.drop_frame">,
): string {
  const format = values["timeline.time_display"];
  if (format === "frames") return String(Math.max(0, flicksToFrame(t, fps)));
  if (format === "seconds") return formatSeconds(t);
  return formatTimecode(t, fps, values["timeline.drop_frame"]);
}

/**
 * How a time shows in a field a person can type in (the Inspector's Start and Duration): the
 * same forms the field reads back, `f:48` for a frame and `2s` for seconds.
 */
export function fieldTime(
  t: number,
  fps: FrameRate,
  values: Pick<PreferenceMap, "timeline.time_display" | "timeline.drop_frame">,
): string {
  const shown = displayTime(t, fps, values);
  return values["timeline.time_display"] === "frames" ? `f:${shown}` : shown;
}

/** The design token each "Height of new tracks" choice is; medium is the engine's default. */
const HEIGHT_TOKEN: Readonly<Record<TrackHeightChoice, string | null>> = {
  small: "--track-sm",
  medium: null,
  large: "--track-lg",
};

/**
 * The height in pixels `track.set_height` gets for a new track, read from the design tokens
 * where `el` is; `null` for medium (the track keeps the default) or without tokens.
 */
export function newTrackHeight(choice: TrackHeightChoice, el: Element | null): number | null {
  const token = HEIGHT_TOKEN[choice];
  const view = el?.ownerDocument.defaultView;
  if (token === null || el === null || view == null) return null;
  const px = Number.parseFloat(view.getComputedStyle(el).getPropertyValue(token));
  return Number.isFinite(px) && px > 0 ? Math.round(px) : null;
}
