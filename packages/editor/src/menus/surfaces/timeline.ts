// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline's menus (WP-2H.13, completed by WP-2A.10), one table per hit zone of the canvas: a
// clip, a clip's edge, an empty lane, a track header, the ruler (with the in/out range's items),
// the playhead and a marker (docs/research/09-context-menus.md §6.4). Every row names a command
// that exists; a row for a command nobody has registered is left out, so a feature WP only adds
// its row. Not here yet, for want of a command: Color label (`item.set_label_color`), the
// marker's Edit… and Convert to chapter (`timeline.update_marker`), Detach audio
// (`audio.extract`), Group, Compound clip, Match frame. TODO(WP-2C.05): the keyframe menu.
import { formatTimecode } from "@cutweave/timeline";
import { canSwap, visualClipIds } from "../../layout/actions.ts";
import { LAYOUT_NAMES } from "../../layout/names.ts";
import { LAYOUTS } from "../../layout/table.ts";
import { isMovingPicture } from "../../media/kinds.ts";
import { activeDocument } from "../../shell/windows.ts";
import { docStore } from "../../state/docStore.ts";
import { selectInOut } from "../../state/selectors.ts";
import { activeTimelineHost } from "../../timeline/activeHost.ts";
import { attributesSource, hasClipboardItems } from "../../timeline/clipboard.ts";
import { onePerGroup, withPartners } from "../../timeline/groups.ts";
import { inOutArgs, markerArgs } from "../../timeline/inOut.ts";
import { itemSelection, trackSelection } from "../adapters.ts";
import type {
  LabelSpec,
  MenuContext,
  MenuRow,
  MenuSurfaceDef,
  MenuTarget,
  SelectionAdapter,
} from "../types.ts";
import type { ContextValues } from "../when.ts";
import {
  activeSequence,
  type ClipFact,
  clipFact,
  clipFacts,
  clipLabel,
  mediaAsset,
  mediaPath,
  trackFact,
} from "./facts.ts";
import { sequenceRate } from "./shared.ts";

const LOCKED = "contextMenu.reason.trackLocked" as const;

/**
 * The clip menus' selection (WP-2A.10): a click on the timeline selects a clip with its linked
 * partners (timeline spec INV-341), and the menu acts on the clips, one per linked group (the
 * engine edits the partners with them, INV-35). A right-click on a clip that is not selected
 * selects it as a click does: with its partners while linked selection is on.
 */
const clipSelection: SelectionAdapter = {
  empty: "keep",
  current: () => onePerGroup(docStore.getState().project, itemSelection.current()),
  set: (ids) =>
    itemSelection.set(
      (activeTimelineHost()?.ui.getState().linkedSelection ?? true)
        ? withPartners(docStore.getState().project, ids)
        : ids,
    ),
};

/** A clip that plays moving pictures: video media on a video track, not a freeze frame. */
const moving = (f: ClipFact): boolean =>
  f.track.kind === "video" &&
  isMovingPicture(mediaAsset(f.clip.media)?.kind) &&
  (f.clip.freeze ?? null) === null;

/** A clip's speed as `num/den` text (`1/1` when the document has none). */
const speedOf = (f: ClipFact): string => `${f.clip.speed?.num ?? 1}/${f.clip.speed?.den ?? 1}`;

/**
 * What a clip menu knows about its target: the track's lock, the clips' enabled state, and
 * whether they all play moving pictures (WP-2D.01: Speed, Reverse and Freeze frame).
 */
export function clipKeys(target: MenuTarget): ContextValues {
  const facts = clipFacts(target.ids);
  return {
    "track.locked": facts.some((f) => f.track.locked),
    "item.enabled": facts.length > 0 && facts.every((f) => f.clip.enabled),
    "item.linked": facts.some((f) => f.clip.linked_group !== null),
    "selection.videoCount": facts.filter((f) => f.track.kind === "video").length,
    "item.movingPicture": facts.length > 0 && facts.every(moving),
    "clipboard.hasItems": hasClipboardItems(),
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
  // One clip of a linked group stands for the group the menu was opened on (WP-2A.10).
  const oneLinked = facts.length === 1 && facts[0]?.clip.linked_group !== null;
  return oneLinked || (alignedOnOwnTracks(facts) && sameGroup(facts))
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

/** The clips' shared speed as `num/den` (`1/1` when the document has none), or `null`. */
function sharedSpeed(ctx: MenuContext): string | null {
  const speeds = new Set(clipFacts(ctx.ids).map(speedOf));
  return speeds.size === 1 ? ([...speeds][0] ?? null) : null;
}

/** Whether every clip of the menu plays backwards. */
function allReversed(ctx: MenuContext): boolean {
  const facts = clipFacts(ctx.ids);
  return facts.length > 0 && facts.every((f) => f.clip.reverse === true);
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

/** One choice of the Speed submenu: `speed` as `num/den`, the label and its place. */
const speedRow = (id: string, speed: string, label: LabelSpec, order: number): MenuRow => ({
  id,
  command: "clip.set_speed",
  group: "speed",
  order,
  radio: true,
  label,
  when: "!track.locked",
  reason: LOCKED,
  checked: (ctx) => sharedSpeed(ctx) === speed,
  args: (ctx) => ({ items: ctx.ids, speed }),
});

/** Freeze frame needs one clip and the playhead on one of its frames. */
const playheadOnClip = (ctx: MenuContext): boolean =>
  ctx.playhead !== null &&
  clipFacts(ctx.ids).some(
    (f) => playheadOf(ctx) >= f.clip.start && playheadOf(ctx) < f.clip.start + f.clip.duration,
  );

// WP-2D.01: Speed ▸, Reverse and Freeze frame, for clips with moving pictures only (sound-only
// clips, stills and freeze frames leave them out).
const speedRows: readonly MenuRow[] = [
  {
    id: "speed",
    group: "speed",
    order: 0,
    visible: "item.movingPicture",
    submenu: {
      label: { key: "contextMenu.item.speed" },
      rows: [
        speedRow("speed-normal", "1/1", { key: "contextMenu.item.speedNormal" }, 0),
        speedRow("speed-half", "1/2", { key: "contextMenu.item.speedHalf" }, 1),
        speedRow("speed-double", "2/1", { key: "contextMenu.item.speedDouble" }, 2),
        {
          command: "timeline.custom_speed",
          group: "custom",
          order: 0,
          when: "!track.locked",
          reason: LOCKED,
        },
      ],
    },
  },
  {
    id: "reverse",
    command: "clip.reverse",
    group: "speed",
    order: 1,
    visible: "item.movingPicture",
    when: "!track.locked",
    reason: LOCKED,
    checked: allReversed,
    args: (ctx) => ({ items: ctx.ids, reverse: !allReversed(ctx) }),
  },
  {
    command: "clip.freeze_frame",
    group: "speed",
    order: 2,
    visible: "item.movingPicture",
    enabledIf: (ctx) => oneClip(ctx) && ctx.keys["track.locked"] !== true && playheadOnClip(ctx),
    reason: splitReason,
    args: (ctx) => ({ item: ctx.ids[0], at: playheadOf(ctx) }),
  },
];
/** Paste attributes reads from the copied clip, which must still be here and not the target. */
const canPasteAttributes = (ctx: MenuContext): boolean => {
  const source = attributesSource();
  return (
    ctx.keys["track.locked"] !== true && source !== null && ctx.ids.some((id) => id !== source)
  );
};

const pasteAttributesReason = (ctx: MenuContext) =>
  ctx.keys["track.locked"] === true
    ? LOCKED
    : attributesSource() === null
      ? ("contextMenu.reason.noCopiedClip" as const)
      : ("contextMenu.reason.oneClip" as const);

/** Replace clip… needs one clip that plays a file (not a freeze frame or a compound clip). */
const canReplace = (ctx: MenuContext): boolean => {
  const only = ctx.ids.length === 1 ? clipFacts(ctx.ids)[0] : undefined;
  return (
    only !== undefined &&
    ctx.keys["track.locked"] !== true &&
    (only.clip.freeze ?? null) === null &&
    mediaAsset(only.clip.media)?.kind !== "sequence_ref"
  );
};

const replaceReason = (ctx: MenuContext) =>
  ctx.keys["track.locked"] === true
    ? LOCKED
    : ctx.ids.length !== 1
      ? ("contextMenu.reason.oneClip" as const)
      : ("contextMenu.reason.cannotReplace" as const);

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
    // WP-2A.10: delete and close the gap (the magnetic track closes it on its own).
    command: "timeline.ripple_delete",
    group: "edit",
    order: 2,
    when: "!track.locked",
    reason: LOCKED,
    args: (ctx) => ({ items: ctx.ids }),
  },
  {
    command: "timeline.replace_clip",
    group: "edit",
    order: 3,
    enabledIf: canReplace,
    reason: replaceReason,
  },
  {
    command: "timeline.cut",
    group: "clipboard",
    order: 0,
    when: "!track.locked",
    reason: LOCKED,
  },
  { command: "timeline.copy_clips", group: "clipboard", order: 1 },
  {
    command: "timeline.paste_attributes",
    group: "clipboard",
    order: 2,
    enabledIf: canPasteAttributes,
    reason: pasteAttributesReason,
  },
  {
    command: "item.duplicate",
    group: "clipboard",
    order: 3,
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
    // The menu names one clip per group: the partners leave the group with it.
    args: (ctx) => ({ items: withPartners(docStore.getState().project, ctx.ids) }),
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
  ...speedRows,
];

function pathOfClip(ctx: MenuContext): string | null {
  const only = ctx.ids.length === 1 ? clipFacts(ctx.ids)[0] : undefined;
  return only === undefined ? null : mediaPath(only.clip.media);
}

/**
 * Whether `ids` are one linked group and nothing else: a click selects a clip with its sound
 * (timeline spec INV-341, WP-2A.10), and the menu still names the one clip.
 */
function oneGroup(ids: readonly string[]): boolean {
  const facts = clipFacts(ids);
  const group = facts[0]?.clip.linked_group ?? null;
  return (
    group !== null &&
    facts.length === ids.length &&
    facts.every((f) => f.clip.linked_group === group)
  );
}

const clipName = (target: MenuTarget) =>
  target.ids.length > 1 && !oneGroup(target.ids)
    ? ({ key: "contextMenu.name.clips", vars: { count: target.ids.length } } as const)
    : ({
        key: "contextMenu.name.clip",
        vars: { name: target.name ?? clipLabel(target.ids[0] ?? "") },
      } as const);

export const TIMELINE_CLIP: MenuSurfaceDef = {
  id: "timeline.clip",
  name: clipName,
  groups: ["edit", "clipboard", "speed", "link", "layout", "state", "reveal"],
  selection: clipSelection,
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
  selection: clipSelection,
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

/** The project the menus read (the same projection the canvas draws). */
const project = () => docStore.getState().project;

/** Where on the timeline the menu opened (the hit's time), or the playhead. */
const timeHere = (ctx: MenuContext): number => ctx.target.place?.time ?? ctx.playhead ?? 0;

/** `timeline.add_marker` at the menu's time ("here"), or at the playhead. */
const markerRow = (id: string, order: number, at: "here" | "playhead", group: string): MenuRow => ({
  id,
  command: "timeline.add_marker",
  group,
  order,
  label: {
    key: at === "here" ? "contextMenu.item.addMarkerHere" : "contextMenu.item.addMarkerAtPlayhead",
  },
  // The M key adds at the playhead; "here" is the click's time.
  ...(at === "here" && { noHint: true }),
  enabledIf: (ctx) =>
    markerArgs(project(), at === "here" ? timeHere(ctx) : (ctx.playhead ?? 0)) !== null,
  reason: "contextMenu.reason.noTrack",
  args: (ctx) => markerArgs(project(), at === "here" ? timeHere(ctx) : (ctx.playhead ?? 0)),
});

/** `timeline.set_in_out` that puts the in or the out point at the menu's time or the playhead. */
const inOutRow = (
  id: string,
  order: number,
  point: "in" | "out",
  at: "here" | "playhead",
  group: string,
): MenuRow => {
  const time = (ctx: MenuContext) => (at === "here" ? timeHere(ctx) : (ctx.playhead ?? 0));
  const label =
    point === "in"
      ? at === "here"
        ? "contextMenu.item.setInHere"
        : "contextMenu.item.setIn"
      : at === "here"
        ? "contextMenu.item.setOutHere"
        : "contextMenu.item.setOut";
  return {
    id,
    command: "timeline.set_in_out",
    group,
    order,
    label: { key: label },
    enabledIf: (ctx) => inOutArgs(project(), point, time(ctx)) !== null,
    reason: "contextMenu.reason.outAtStart",
    args: (ctx) => inOutArgs(project(), point, time(ctx)),
  };
};

/** The sequence's in and out are set. */
const hasRange = (): boolean => selectInOut(project()) !== null;

/** The active sequence's id, for commands that name it. */
const sequenceArg = () => ({ sequence: activeSequence()?.id });

/** The ruler's in/out rows: clear them, lift or extract what is between (WP-2A.03). */
const rangeRows = (): MenuRow[] => [
  {
    command: "timeline.clear_in_out",
    group: "range",
    order: 0,
    enabledIf: hasRange,
    reason: "contextMenu.reason.noInOut",
    args: sequenceArg,
  },
  {
    command: "timeline.lift",
    group: "range",
    order: 1,
    enabledIf: hasRange,
    reason: "contextMenu.reason.noInOut",
    args: sequenceArg,
  },
  {
    command: "timeline.extract",
    group: "range",
    order: 2,
    enabledIf: hasRange,
    reason: "contextMenu.reason.noInOut",
    args: sequenceArg,
  },
];

/** A gap between two clips at the lane point: some clip on the track starts after it. */
function gapAt(target: MenuTarget): boolean {
  const time = target.place?.time;
  const fact = trackFact(target.place?.trackId ?? "");
  if (time === undefined || fact === null) return false;
  const items = Object.values(fact.track.items);
  const inside = items.some((i) => time >= i.start && time < i.start + i.duration);
  return !inside && items.some((i) => i.start > time);
}

export const TIMELINE_LANE: MenuSurfaceDef = {
  id: "timeline.lane",
  name: () => ({ key: "contextMenu.name.lane" }),
  groups: ["playhead", "clipboard", "marker", "gap", "select", "tracks", "view"],
  selection: itemSelection,
  keys: (target) => ({
    "track.locked": trackFact(target.place?.trackId ?? "")?.track.locked === true,
    "clipboard.hasItems": hasClipboardItems(),
  }),
  rows: [
    moveHere,
    {
      command: "timeline.paste_here",
      group: "clipboard",
      order: 0,
      enabledIf: (ctx) =>
        ctx.keys["clipboard.hasItems"] === true && ctx.keys["track.locked"] !== true,
      reason: (ctx) =>
        ctx.keys["track.locked"] === true ? LOCKED : ("contextMenu.reason.clipboardEmpty" as const),
    },
    markerRow("add-marker-here", 0, "here", "marker"),
    {
      // WP-2A.03: closes the gap under the pointer; shown only on a gap between clips.
      command: "timeline.delete_gap",
      group: "gap",
      order: 0,
      visibleIf: (ctx) => gapAt(ctx.target),
      when: "!track.locked",
      reason: LOCKED,
      args: (ctx) => ({ track: ctx.target.place?.trackId, at: timeHere(ctx) }),
    },
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
    { command: "timeline.select_after_here", group: "select", order: 1 },
    ...addRows("tracks"),
    { command: "timeline.zoom_to_fit", group: "view", order: 0 },
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

/** A length token in CSS px, as the timeline lays its rows out with it. */
function tokenPx(name: string): number {
  const doc = activeDocument();
  const value = doc.defaultView?.getComputedStyle(doc.documentElement).getPropertyValue(name);
  return Number.parseFloat(value ?? "") || 0;
}

type Height = "sm" | "md" | "lg";

/** Which of the three sizes a track's height (`track.height`, CSS px; `null`: default) is. */
function heightOf(height: number | null | undefined): Height {
  if (height === null || height === undefined) return "md";
  if (height <= tokenPx("--track-sm")) return "sm";
  if (height >= tokenPx("--track-lg")) return "lg";
  return "md";
}

const heightRow = (size: Height, order: number, label: LabelSpec): MenuRow => ({
  id: `height-${size}`,
  command: "track.set_height",
  group: "height",
  order,
  radio: true,
  label,
  checked: (ctx) => heightOf(trackOf(ctx)?.height) === size,
  args: (ctx) => ({
    track: ctx.ids[0],
    height: size === "md" ? null : tokenPx(size === "sm" ? "--track-sm" : "--track-lg"),
  }),
});

/** `track.reorder` one place up or down (0 is the top). */
const moveRow = (id: string, order: number, by: -1 | 1, label: LabelSpec): MenuRow => ({
  id,
  command: "track.reorder",
  group: "tracks",
  order,
  label,
  enabledIf: (ctx) => {
    const fact = trackFact(ctx.ids[0] ?? "");
    if (fact === null) return false;
    const next = fact.index + by;
    return next >= 0 && next < fact.sequence.sequence.track_order.length;
  },
  reason: by < 0 ? "contextMenu.reason.trackAtTop" : "contextMenu.reason.trackAtBottom",
  args: (ctx) => ({ track: ctx.ids[0], index: (trackFact(ctx.ids[0] ?? "")?.index ?? 0) + by }),
});

export const TIMELINE_TRACK: MenuSurfaceDef = {
  id: "timeline.track",
  name: (target) => ({
    key: "contextMenu.name.track",
    vars: { name: target.name ?? trackFact(target.ids[0] ?? "")?.track.name ?? "" },
  }),
  groups: ["name", "flags", "height", "tracks", "remove"],
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
    {
      // WP-2A.02: the magnetic main track (one per sequence; video tracks).
      id: "magnetic",
      command: "track.set_magnetic",
      group: "flags",
      order: 4,
      enabledIf: (ctx) => trackOf(ctx)?.kind === "video",
      reason: "contextMenu.reason.mainTrackVideo",
      label: { key: "contextMenu.item.magneticTrack" },
      checked: (ctx) => trackOf(ctx)?.magnetic === true,
      args: (ctx) => ({ track: ctx.ids[0], magnetic: trackOf(ctx)?.magnetic !== true }),
    },
    {
      id: "height",
      group: "height",
      order: 0,
      submenu: {
        label: { key: "contextMenu.item.trackHeight" },
        rows: [
          heightRow("sm", 0, { key: "contextMenu.item.heightSmall" }),
          heightRow("md", 1, { key: "contextMenu.item.heightMedium" }),
          heightRow("lg", 2, { key: "contextMenu.item.heightLarge" }),
        ],
      },
    },
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
    moveRow("move-up", 3, -1, { key: "contextMenu.item.moveTrackUp" }),
    moveRow("move-down", 4, 1, { key: "contextMenu.item.moveTrackDown" }),
    { command: "timeline.delete_track", group: "remove", order: 0 },
  ],
};

/** The time a timeline menu opened at, as a timecode, for "Copy timecode". */
function timecodeAt(ctx: MenuContext): string {
  const rate = sequenceRate() ?? { num: 30, den: 1 };
  return formatTimecode(ctx.target.place?.time ?? ctx.playhead ?? 0, rate, true);
}

const navigateRows = (): MenuRow[] => [
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
  groups: ["playhead", "marker", "range", "navigate", "view", "copy"],
  rows: [
    moveHere,
    markerRow("add-marker-here", 0, "here", "marker"),
    inOutRow("set-in-here", 1, "in", "here", "marker"),
    inOutRow("set-out-here", 2, "out", "here", "marker"),
    ...rangeRows(),
    ...navigateRows(),
    {
      // Go to marker ▸: the sequence's markers in time order (none: the row is left out).
      id: "go-to-marker",
      group: "navigate",
      order: 2,
      submenu: { label: { key: "contextMenu.item.goToMarkerMenu" }, rows: [], more: markerRows },
    },
    { command: "timeline.zoom_to_fit", group: "view", order: 0 },
    { command: "timeline.zoom_to_selection", group: "view", order: 1 },
  ],
};

/** At most this many markers in Go to marker ▸ (the menu stays about a screen tall). */
const MARKER_ROWS = 12;

/** One row per sequence marker, by time: its name (or "Marker") and its timecode. */
function markerRows(): readonly MenuRow[] {
  const found = activeSequence();
  const rate = sequenceRate() ?? { num: 30, den: 1 };
  return Object.values(found?.sequence.markers ?? {})
    .sort((a, b) => a.t - b.t)
    .slice(0, MARKER_ROWS)
    .map(
      (marker, index): MenuRow => ({
        id: `go-to-${marker.id}`,
        command: "timeline.go_to_marker",
        group: "markers",
        order: index,
        noHint: true,
        label: {
          key: "contextMenu.item.markerAt",
          vars: {
            name: marker.label === "" ? "-" : marker.label,
            time: formatTimecode(marker.t, rate, true),
          },
        },
        args: () => ({ t: marker.t }),
      }),
    );
}

export const TIMELINE_PLAYHEAD: MenuSurfaceDef = {
  id: "timeline.playhead",
  name: () => ({ key: "contextMenu.name.playhead" }),
  groups: ["edit", "marker", "navigate", "copy"],
  rows: [
    { command: "timeline.split_at_playhead", group: "edit", order: 0 },
    markerRow("add-marker", 0, "playhead", "marker"),
    inOutRow("set-in", 1, "in", "playhead", "marker"),
    inOutRow("set-out", 2, "out", "playhead", "marker"),
    ...navigateRows(),
  ],
};

/** A sequence marker (its pin in the ruler): go to it, or delete it. */
export const TIMELINE_MARKER: MenuSurfaceDef = {
  id: "timeline.marker",
  name: (target) => ({
    key: "contextMenu.name.marker",
    vars: { name: target.name === undefined || target.name === "" ? "-" : target.name },
  }),
  groups: ["navigate", "remove"],
  rows: [
    { command: "timeline.go_to_marker", group: "navigate", order: 0 },
    {
      command: "timeline.remove_marker",
      group: "remove",
      order: 0,
      args: (ctx) => ({ marker: ctx.ids[0] }),
    },
  ],
};
