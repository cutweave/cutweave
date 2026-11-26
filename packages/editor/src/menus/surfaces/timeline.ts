// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline's menus (WP-2H.13), one table per hit zone of the canvas: a clip, an empty lane, a
// track header, the ruler and the playhead. WP-2A.10 and the work packages that add timeline
// commands add their rows here; a row for a command nobody has registered is left out, so a
// feature WP only adds its row.
// TODO(WP-2A.03): Ripple delete, Delete gap. TODO(WP-2A.06): Cut, Copy, Paste, Paste attributes,
// Duplicate. TODO(WP-2A.11): Replace clip. TODO(WP-2D.01):
// Speed, Reverse, Freeze frame. TODO(WP-2A.02): markers, in and out, Make magnetic, track
// height, Move up and down, Color label. TODO(WP-2A.09): Zoom to fit and Zoom to selection.
// TODO(WP-2C.05): the keyframe menu. The edge menu (WP-2A.04) trims, ripple trims and rolls the
// edge to the playhead, and extends it to the next clip.
import { formatTimecode } from "@cutweave/timeline";
import { itemSelection, trackSelection } from "../adapters.ts";
import type { LabelSpec, MenuContext, MenuRow, MenuSurfaceDef, MenuTarget } from "../types.ts";
import type { ContextValues } from "../when.ts";
import {
  activeSequence,
  type ClipFact,
  clipFact,
  clipFacts,
  clipLabel,
  mediaPath,
  trackFact,
} from "./facts.ts";
import { sequenceRate } from "./shared.ts";
import { canSwap, visualClipIds } from "../../layout/actions.ts";
import { LAYOUT_NAMES } from "../../layout/names.ts";
import { LAYOUTS } from "../../layout/table.ts";

const LOCKED = "contextMenu.reason.trackLocked" as const;

/** What a clip menu knows about its target: the track's lock and the clips' enabled state. */
export function clipKeys(target: MenuTarget): ContextValues {
  const facts = clipFacts(target.ids);
  return {
    "track.locked": facts.some((f) => f.track.locked),
    "item.enabled": facts.length > 0 && facts.every((f) => f.clip.enabled),
    "item.linked": facts.some((f) => f.clip.linked_group !== null),
    "selection.videoCount": facts.filter((f) => f.track.kind === "video").length,
  };
}

// ----- links and attachments (WP-2A.05) ----------------------------------------------------------

/** The clips of the menu's selection start and end together, each on its own track. */
function alignedOnOwnTracks(facts: readonly ClipFact[]): boolean {
  const [first] = facts;
  return (
    first !== undefined &&
    facts.length >= 2 &&
    facts.every(
      (f) => f.clip.start === first.clip.start && f.clip.duration === first.clip.duration,
    ) &&
    new Set(facts.map((f) => f.track.id)).size === facts.length
  );
}

const sameGroup = (facts: readonly ClipFact[]): boolean =>
  facts.every(
    (f) => f.clip.linked_group !== null && f.clip.linked_group === facts[0]?.clip.linked_group,
  );

const canLink = (ctx: MenuContext): boolean => {
  const facts = clipFacts(ctx.ids);
  return (
    facts.length === ctx.ids.length &&
    ctx.keys["track.locked"] !== true &&
    alignedOnOwnTracks(facts) &&
    !sameGroup(facts)
  );
};

const linkReason = (ctx: MenuContext) => {
  if (ctx.keys["track.locked"] === true) return LOCKED;
  const facts = clipFacts(ctx.ids);
  return alignedOnOwnTracks(facts) && sameGroup(facts)
    ? ("contextMenu.reason.alreadyLinked" as const)
    : ("contextMenu.reason.notAligned" as const);
};

/** The main-track clip a clip menu's selection can be attached to: the one all of them start in. */
function mainClipFor(ids: readonly string[]): string | null {
  const found = activeSequence();
  const facts = clipFacts(ids);
  const main = Object.values(found?.sequence.tracks ?? {}).find((t) => t.magnetic);
  if (main === undefined || facts.length === 0) return null;
  const parent = Object.values(main.items).find((i) =>
    facts.every((f) => f.clip.start >= i.start && f.clip.start < i.start + i.duration),
  );
  const onMain = facts.some((f) => f.track.id === main.id);
  return parent === undefined || onMain ? null : parent.id;
}

const canAttach = (ctx: MenuContext): boolean =>
  ctx.keys["track.locked"] !== true && mainClipFor(ctx.ids) !== null;

const attachReason = (ctx: MenuContext) =>
  ctx.keys["track.locked"] === true ? LOCKED : ("contextMenu.reason.noMainClip" as const);

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

// ----- layouts (WP-2C.08) ------------------------------------------------------------------------

/** One row per layout of the table for each clip count; only the rows for the selection show. */
const layoutRows: readonly MenuRow[] = LAYOUTS.flatMap((layout, order) => {
  const name = LAYOUT_NAMES[layout.id];
  return name === undefined
    ? []
    : [
        {
          id: `layout-${layout.id}`,
          command: "layout.apply",
          group: "layout",
          order,
          visible: `selection.videoCount == ${layout.count}`,
          label: { key: name },
          enabledIf: (ctx: MenuContext) => ctx.keys["track.locked"] !== true,
          reason: LOCKED,
          args: (ctx: MenuContext) => ({ ids: visualClipIds(ctx.ids), layout: layout.id }),
        } satisfies MenuRow,
      ];
});

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
    command: "item.link",
    group: "link",
    order: 0,
    enabledIf: canLink,
    reason: linkReason,
    args: (ctx) => ({ items: ctx.ids }),
  },
  {
    command: "item.unlink",
    group: "link",
    order: 1,
    enabledIf: (ctx) => ctx.keys["item.linked"] === true && ctx.keys["track.locked"] !== true,
    reason: (ctx) =>
      ctx.keys["track.locked"] === true ? LOCKED : ("contextMenu.reason.notLinked" as const),
    args: (ctx) => ({ items: ctx.ids }),
  },
  {
    command: "item.attach",
    group: "link",
    order: 2,
    enabledIf: canAttach,
    reason: attachReason,
    args: (ctx) => ({ items: ctx.ids, parent: mainClipFor(ctx.ids) }),
  },
  {
    command: "item.detach",
    group: "link",
    order: 3,
    enabledIf: (ctx) =>
      ctx.keys["track.locked"] !== true &&
      clipFacts(ctx.ids).some((f) => (f.clip.attached_to ?? null) !== null),
    reason: (ctx) =>
      ctx.keys["track.locked"] === true ? LOCKED : ("contextMenu.reason.notAttached" as const),
    args: (ctx) => ({ items: ctx.ids }),
  },
  {
    id: "layout",
    group: "layout",
    order: 0,
    submenu: { label: { key: "contextMenu.item.layout" }, rows: layoutRows },
  },
  {
    id: "layout-swap",
    command: "layout.swap",
    group: "layout",
    order: 1,
    visible: "selection.videoCount == 2",
    enabledIf: (ctx) => ctx.keys["track.locked"] !== true && canSwap(ctx.ids),
    reason: (ctx) => (ctx.keys["track.locked"] === true ? LOCKED : "contextMenu.reason.layoutSwap"),
    args: (ctx) => ({ ids: visualClipIds(ctx.ids) }),
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
    id: "reset-props",
    command: "item.reset_props",
    group: "state",
    order: 2,
    enabledIf: (ctx) => ctx.ids.length > 0 && ctx.keys["track.locked"] !== true,
    reason: LOCKED,
    label: { key: "contextMenu.item.resetProperties" },
    args: (ctx) => ({ items: ctx.ids }),
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
  groups: ["edit", "link", "layout", "state", "reveal"],
  selection: itemSelection,
  keys: clipKeys,
  rows: clipRows,
};

// ----- a clip's edge (WP-2A.04) ------------------------------------------------------------------

type Edge = "start" | "end";

/** The edge a right-click on a trim handle hit: the left one is the start, the right one the end. */
function edgeOf(ctx: MenuContext): Edge {
  const zone = (ctx.target.data as { readonly zone?: string } | undefined)?.zone;
  return zone === "left-edge" ? "start" : "end";
}

/** The clip whose edge the menu is on: the one hit, whatever else is selected. */
function edgeClip(ctx: MenuContext): ClipFact | null {
  const id = ctx.target.ids[0];
  return id === undefined ? null : clipFact(id);
}

const endOf = (clip: { readonly start: number; readonly duration: number }) =>
  clip.start + clip.duration;

/** The nearest clip of the same track on the edge's side, and whether it touches the edge. */
function neighbour(fact: ClipFact, edge: Edge) {
  // One pass: a track can hold thousands of clips and a menu reads this several times.
  let near: ClipFact["clip"] | undefined;
  for (const i of Object.values(fact.track.items)) {
    if (i.id === fact.clip.id) continue;
    const closer =
      edge === "end"
        ? i.start >= endOf(fact.clip) && (near === undefined || i.start < near.start)
        : endOf(i) <= fact.clip.start && (near === undefined || endOf(i) > endOf(near));
    if (closer) near = i;
  }
  if (near === undefined) return null;
  const touches =
    edge === "end" ? near.start === endOf(fact.clip) : endOf(near) === fact.clip.start;
  return { clip: near, touches };
}

/** The playhead moves the edge somewhere: past the other edge of the clip, not onto this one. */
function playheadMovesEdge(ctx: MenuContext, fact: ClipFact): boolean {
  if (ctx.playhead === null) return false;
  const p = ctx.playhead;
  return edgeOf(ctx) === "end"
    ? p > fact.clip.start && p !== endOf(fact.clip)
    : p < endOf(fact.clip) && p !== fact.clip.start;
}

const trimEnabled = (ctx: MenuContext): boolean => {
  const fact = edgeClip(ctx);
  return fact !== null && !fact.track.locked && playheadMovesEdge(ctx, fact);
};

const trimReason = (ctx: MenuContext) =>
  edgeClip(ctx)?.track.locked === true ? LOCKED : "contextMenu.reason.playheadAtEdge";

/** The two clips of a roll on this edge, `[left, right]`, when a clip touches it. */
function rollPair(ctx: MenuContext): readonly [string, string] | null {
  const fact = edgeClip(ctx);
  const next = fact === null ? null : neighbour(fact, edgeOf(ctx));
  if (fact === null || next === null || !next.touches) return null;
  return edgeOf(ctx) === "end" ? [fact.clip.id, next.clip.id] : [next.clip.id, fact.clip.id];
}

function rollEnabled(ctx: MenuContext): boolean {
  const pair = rollPair(ctx);
  const fact = edgeClip(ctx);
  if (pair === null || fact === null || fact.track.locked || ctx.playhead === null) return false;
  const [left, right] = pair.map((id) => clipFact(id)?.clip);
  if (left === undefined || right === undefined) return false;
  const p = ctx.playhead;
  return p > left.start && p < endOf(right) && p !== endOf(left);
}

const rollReason = (ctx: MenuContext) =>
  edgeClip(ctx)?.track.locked === true
    ? LOCKED
    : rollPair(ctx) === null
      ? "contextMenu.reason.noAdjacentClip"
      : "contextMenu.reason.playheadAtEdge";

/** Where "Extend" takes the edge: the next clip's start, or the previous clip's end. */
function extendTo(ctx: MenuContext): number | null {
  const fact = edgeClip(ctx);
  const next = fact === null ? null : neighbour(fact, edgeOf(ctx));
  if (next === null || next.touches) return null;
  return edgeOf(ctx) === "end" ? next.clip.start : endOf(next.clip);
}

const playheadOrZero = (ctx: MenuContext): number => ctx.playhead ?? 0;

// Every row passes `clamp: true`: an edge stops where the media or a neighbour stops it, as when
// it is dragged, instead of failing.
const edgeRows: readonly MenuRow[] = [
  {
    id: "trim-to-playhead",
    command: "clip.trim",
    group: "trim",
    order: 0,
    label: { key: "contextMenu.item.trimToPlayhead" },
    enabledIf: trimEnabled,
    reason: trimReason,
    args: (ctx) => ({
      item: ctx.target.ids[0],
      edge: edgeOf(ctx),
      to: playheadOrZero(ctx),
      clamp: true,
    }),
  },
  {
    id: "ripple-trim-to-playhead",
    command: "clip.ripple_trim",
    group: "trim",
    order: 1,
    label: { key: "contextMenu.item.rippleTrimToPlayhead" },
    enabledIf: trimEnabled,
    reason: trimReason,
    args: (ctx) => ({
      item: ctx.target.ids[0],
      edge: edgeOf(ctx),
      to: playheadOrZero(ctx),
      clamp: true,
    }),
  },
  {
    id: "roll-to-playhead",
    command: "clip.roll",
    group: "trim",
    order: 2,
    label: { key: "contextMenu.item.rollToPlayhead" },
    enabledIf: rollEnabled,
    reason: rollReason,
    args: (ctx) => {
      const [left, right] = rollPair(ctx) ?? [];
      return { left, right, to: playheadOrZero(ctx), clamp: true };
    },
  },
  {
    id: "extend",
    command: "clip.trim",
    group: "extend",
    order: 0,
    label: (ctx) => ({
      key:
        edgeOf(ctx) === "end"
          ? "contextMenu.item.extendToNext"
          : "contextMenu.item.extendToPrevious",
    }),
    enabledIf: (ctx) => edgeClip(ctx)?.track.locked !== true && extendTo(ctx) !== null,
    reason: (ctx) => (edgeClip(ctx)?.track.locked === true ? LOCKED : "contextMenu.reason.noGap"),
    args: (ctx) => ({
      item: ctx.target.ids[0],
      edge: edgeOf(ctx),
      to: extendTo(ctx) ?? 0,
      clamp: true,
    }),
  },
];

/** A clip's edge: trim, ripple trim or roll it to the playhead, or extend it to the next clip. */
export const TIMELINE_EDGE: MenuSurfaceDef = {
  id: "timeline.edge",
  name: (target) => ({
    key: "contextMenu.name.edge",
    vars: { name: target.name ?? clipLabel(target.ids[0] ?? "") },
  }),
  groups: ["trim", "extend"],
  selection: itemSelection,
  keys: clipKeys,
  rows: edgeRows,
};

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
