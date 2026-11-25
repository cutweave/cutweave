// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline's menus (WP-2H.13), one table per hit zone of the canvas: a clip, an empty lane, a
// track header, the ruler and the playhead. WP-2A.10 and the work packages that add timeline
// commands add their rows here; a row for a command nobody has registered is left out, so a
// feature WP only adds its row.
// TODO(WP-2A.03): Ripple delete, Delete gap. TODO(WP-2A.06): Cut, Copy, Paste, Paste attributes,
// Duplicate. TODO(WP-2A.05): Link, Unlink, Group. TODO(WP-2A.11): Replace clip. TODO(WP-2D.01):
// Speed, Reverse, Freeze frame. TODO(WP-2A.02): markers, in and out, Make magnetic, track
// height, Move up and down, Color label. TODO(WP-2A.04): the edge menu (Trim to playhead,
// Extend to next clip), until then a clip edge opens the clip menu. TODO(WP-2A.09): Zoom to fit
// and Zoom to selection. TODO(WP-2C.05): the keyframe menu.
import { formatTimecode } from "@cutweave/timeline";
import { itemSelection, trackSelection } from "../adapters.ts";
import type { LabelSpec, MenuContext, MenuRow, MenuSurfaceDef, MenuTarget } from "../types.ts";
import type { ContextValues } from "../when.ts";
import { clipFacts, clipLabel, mediaPath, trackFact } from "./facts.ts";
import { sequenceRate } from "./shared.ts";

const LOCKED = "contextMenu.reason.trackLocked" as const;

/** What a clip menu knows about its target: the track's lock and the clips' enabled state. */
export function clipKeys(target: MenuTarget): ContextValues {
  const facts = clipFacts(target.ids);
  return {
    "track.locked": facts.some((f) => f.track.locked),
    "item.enabled": facts.length > 0 && facts.every((f) => f.clip.enabled),
  };
}

/** The playhead the menu acts on (Split cuts there, not where the pointer was). */
const playheadOf = (ctx: MenuContext): number => ctx.playhead ?? 0;

const oneClip = (ctx: MenuContext): boolean => ctx.ids.length === 1;

const clipReason = (ctx: MenuContext) =>
  ctx.keys["track.locked"] === true ? LOCKED : "contextMenu.reason.oneClip";

const splitReason = (ctx: MenuContext) =>
  ctx.keys["track.locked"] === true
    ? LOCKED
    : ctx.ids.length !== 1
      ? "contextMenu.reason.oneClip"
      : "contextMenu.reason.playheadOutside";

const clipRows: readonly MenuRow[] = [
  {
    command: "clip.split",
    group: "edit",
    order: 0,
    enabledIf: (ctx) =>
      ctx.ids.length === 1 &&
      ctx.keys["track.locked"] !== true &&
      ctx.playhead !== null &&
      clipFacts(ctx.ids).some(
        (f) => playheadOf(ctx) > f.clip.start && playheadOf(ctx) < f.clip.start + f.clip.duration,
      ),
    reason: splitReason,
    args: (ctx) => ({ item: ctx.ids[0], at: playheadOf(ctx) }),
  },
  {
    command: "item.delete",
    group: "edit",
    order: 1,
    when: "!track.locked",
    reason: LOCKED,
    args: (ctx) => ({ items: ctx.ids }),
  },
  {
    id: "enabled",
    command: "item.set_props",
    group: "state",
    order: 0,
    enabledIf: (ctx) => oneClip(ctx) && ctx.keys["track.locked"] !== true,
    reason: clipReason,
    label: (ctx) => ({
      key:
        ctx.keys["item.enabled"] === true
          ? "contextMenu.item.disableClip"
          : "contextMenu.item.enableClip",
    }),
    args: (ctx) => ({
      item: ctx.ids[0],
      props: { enabled: ctx.keys["item.enabled"] !== true },
    }),
  },
  {
    command: "timeline.rename_clip",
    group: "state",
    order: 1,
    enabledIf: (ctx) => oneClip(ctx) && ctx.keys["track.locked"] !== true,
    reason: clipReason,
  },
  {
    command: "timeline.reveal_in_bin",
    group: "reveal",
    order: 0,
    enabledIf: oneClip,
    reason: "contextMenu.reason.oneClip",
  },
  {
    id: "copy-path",
    command: "host.copy_text",
    group: "reveal",
    order: 1,
    label: { key: "contextMenu.item.copyPath" },
    enabledIf: (ctx) => pathOfClip(ctx) !== null,
    reason: "contextMenu.reason.noFile",
    args: (ctx) => ({ text: pathOfClip(ctx) ?? "" }),
  },
];

function pathOfClip(ctx: MenuContext): string | null {
  const only = ctx.ids.length === 1 ? clipFacts(ctx.ids)[0] : undefined;
  return only === undefined ? null : mediaPath(only.clip.media);
}

const clipName = (target: MenuTarget) =>
  target.ids.length > 1
    ? ({ key: "contextMenu.name.clips", vars: { count: target.ids.length } } as const)
    : ({
        key: "contextMenu.name.clip",
        vars: { name: target.name ?? clipLabel(target.ids[0] ?? "") },
      } as const);

export const TIMELINE_CLIP: MenuSurfaceDef = {
  id: "timeline.clip",
  name: clipName,
  groups: ["edit", "state", "reveal"],
  selection: itemSelection,
  keys: clipKeys,
  rows: clipRows,
};

/** A clip's edge opens the clip menu until the trim items of WP-2A.04 give it its own. */
export const TIMELINE_EDGE: MenuSurfaceDef = { ...TIMELINE_CLIP, id: "timeline.edge" };

/** `track.add` next to the track the menu is on: above it, or below it; the same kind. */
const addTrack =
  (below: boolean) =>
  (ctx: MenuContext): unknown => {
    const id = ctx.target.place?.trackId ?? ctx.ids[0];
    const fact = id === undefined ? null : trackFact(id);
    if (fact === null) return undefined;
    return {
      sequence: fact.sequence.id,
      kind: fact.track.kind,
      index: fact.index + (below ? 1 : 0),
    };
  };

const hasTrack = (ctx: MenuContext): boolean =>
  trackFact(ctx.target.place?.trackId ?? ctx.ids[0] ?? "") !== null;

const addRows = (group: string): MenuRow[] => [
  {
    id: "add-above",
    command: "track.add",
    group,
    order: 0,
    label: { key: "contextMenu.item.addTrackAbove" },
    enabledIf: hasTrack,
    reason: "contextMenu.reason.noTrack",
    args: addTrack(false),
  },
  {
    id: "add-below",
    command: "track.add",
    group,
    order: 1,
    label: { key: "contextMenu.item.addTrackBelow" },
    enabledIf: hasTrack,
    reason: "contextMenu.reason.noTrack",
    args: addTrack(true),
  },
];

const moveHere: MenuRow = {
  command: "timeline.move_playhead_here",
  group: "playhead",
  order: 0,
};

export const TIMELINE_LANE: MenuSurfaceDef = {
  id: "timeline.lane",
  name: () => ({ key: "contextMenu.name.lane" }),
  groups: ["playhead", "select", "tracks"],
  selection: itemSelection,
  keys: (target) => ({
    "track.locked": trackFact(target.place?.trackId ?? "")?.track.locked === true,
  }),
  rows: [
    moveHere,
    {
      command: "timeline.select_all_on_track",
      group: "select",
      order: 0,
      enabledIf: (ctx) => {
        const fact = trackFact(ctx.target.place?.trackId ?? "");
        return fact !== null && Object.keys(fact.track.items).length > 0;
      },
      reason: "contextMenu.reason.emptyTrack",
    },
    ...addRows("tracks"),
  ],
};

const trackSet = (patch: (track: ReturnType<typeof trackOf>) => object) => (ctx: MenuContext) => ({
  track: ctx.ids[0],
  ...patch(trackOf(ctx)),
});

function trackOf(ctx: MenuContext) {
  return trackFact(ctx.ids[0] ?? "")?.track;
}

const flagRow = (
  id: string,
  order: number,
  flag: "muted" | "solo" | "hidden" | "locked",
  on: LabelSpec,
  off: LabelSpec,
): MenuRow => ({
  id,
  command: "track.set",
  group: "flags",
  order,
  label: (ctx) => (trackOf(ctx)?.[flag] === true ? off : on),
  checked: (ctx) => trackOf(ctx)?.[flag] === true,
  args: trackSet((track) => ({ [flag]: track?.[flag] !== true })),
});

export const TIMELINE_TRACK: MenuSurfaceDef = {
  id: "timeline.track",
  name: (target) => ({
    key: "contextMenu.name.track",
    vars: { name: target.name ?? trackFact(target.ids[0] ?? "")?.track.name ?? "" },
  }),
  groups: ["name", "flags", "tracks", "remove"],
  selection: trackSelection,
  keys: (target) => ({
    "track.locked": trackFact(target.ids[0] ?? "")?.track.locked === true,
  }),
  rows: [
    { command: "timeline.rename_track", group: "name", order: 0 },
    flagRow(
      "mute",
      0,
      "muted",
      { key: "contextMenu.item.muteTrack" },
      { key: "contextMenu.item.unmuteTrack" },
    ),
    flagRow(
      "solo",
      1,
      "solo",
      { key: "contextMenu.item.soloTrack" },
      { key: "contextMenu.item.unsoloTrack" },
    ),
    flagRow(
      "hide",
      2,
      "hidden",
      { key: "contextMenu.item.hideTrack" },
      { key: "contextMenu.item.showTrack" },
    ),
    flagRow(
      "lock",
      3,
      "locked",
      { key: "contextMenu.item.lockTrack" },
      { key: "contextMenu.item.unlockTrack" },
    ),
    ...addRows("tracks"),
    {
      command: "timeline.select_all_on_track",
      group: "tracks",
      order: 2,
      enabledIf: (ctx) => {
        const fact = trackFact(ctx.ids[0] ?? "");
        return fact !== null && Object.keys(fact.track.items).length > 0;
      },
      reason: "contextMenu.reason.emptyTrack",
    },
    { command: "timeline.delete_track", group: "remove", order: 0 },
  ],
};

/** The time a timeline menu opened at, as a timecode, for "Copy timecode". */
function timecodeAt(ctx: MenuContext): string {
  const rate = sequenceRate() ?? { num: 30, den: 1 };
  return formatTimecode(ctx.target.place?.time ?? ctx.playhead ?? 0, rate, true);
}

const timeRows = (): MenuRow[] => [
  moveHere,
  { command: "transport.to_start", group: "navigate", order: 0 },
  { command: "transport.to_end", group: "navigate", order: 1 },
  {
    id: "copy-timecode",
    command: "host.copy_text",
    group: "copy",
    order: 0,
    label: { key: "contextMenu.item.copyTimecode" },
    args: (ctx) => ({ text: timecodeAt(ctx) }),
  },
];

export const TIMELINE_RULER: MenuSurfaceDef = {
  id: "timeline.ruler",
  name: () => ({ key: "contextMenu.name.ruler" }),
  groups: ["playhead", "navigate", "copy"],
  rows: timeRows(),
};

export const TIMELINE_PLAYHEAD: MenuSurfaceDef = {
  id: "timeline.playhead",
  name: () => ({ key: "contextMenu.name.playhead" }),
  groups: ["navigate", "copy"],
  rows: timeRows().filter((row) => row !== moveHere),
};
