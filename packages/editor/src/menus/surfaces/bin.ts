// SPDX-License-Identifier: GPL-3.0-or-later
// The media bin's menus (WP-2H.13): a card (one or several selected), the background and the empty
// bin, and a filter chip. Rows name commands that exist today; what later work packages add is
// listed in the TODO below, not stubbed.
// TODO(WP-2B.01): New timeline from selection.
// Each work package adds its row to these tables. Add tag… and Set rating ▸ (WP-2F.06b) are on
// a card, for one or several. Collections (WP-2F.06c): Add to collection ▸ (the manual
// collections and New collection…) and, while a manual collection is open, Remove from
// collection on a card; New collection… and Save as smart collection… on the background; a
// collection row of the sidebar has its own surface, `bin.collection`. Relink… (WP-2F.03) shows on a card whose file is missing. Regenerate proxies
// and Delete proxies (WP-2F.03b) show on video. Folders (WP-2F.08): New folder, Rename, Move to, Open, Delete and Select all.
// #215: Duration… on an image card (the length its next insert uses) and Proxy ▸ Auto / Always /
// Never on video (`media.set_proxy_mode`, ADR-0027).
import type { MediaId, ProxyMode } from "@cutweave/protocol";
import { snapToFrame } from "@cutweave/timeline";
import { placeable } from "../../layout/actions.ts";
import { LAYOUT_NAMES } from "../../layout/names.ts";
import { LAYOUTS } from "../../layout/table.ts";
import { binStore, SORT_CHOICES } from "../../media/binStore.ts";
import { selectMediaCards, selectMediaUsage } from "../../media/cards.ts";
import { currentSmartFilter } from "../../media/collectionCommands.ts";
import {
  isManualCollection,
  MAX_COLLECTION_MEMBERS,
  notYetOn,
  selectCollections,
} from "../../media/collections.ts";
import { healthStore } from "../../media/health.ts";
import { insertDurationFor } from "../../media/insertDuration.ts";
import { isMovingPicture } from "../../media/kinds.ts";
import { layoutStore } from "../../media/layoutStore.ts";
import { canHaveProxy, hasProxy, proxyStore } from "../../media/proxies.ts";
import { currentOpenCollection } from "../../media/sidebar.ts";
import { MAX_TAG_MEDIA, RATINGS } from "../../media/tags.ts";
import { prefsStore } from "../../prefs/prefsStore.ts";
import { docStore } from "../../state/docStore.ts";
import { viewStore } from "../../state/viewStore.ts";
import { canReplaceWith, pickedClip } from "../../timeline/replacePick.ts";
import { folderSelection, mediaSelection } from "../adapters.ts";
import type { LabelSpec, MenuContext, MenuRow, MenuSurfaceDef } from "../types.ts";
import type { ContextValues } from "../when.ts";
import { activeSequence, clipFact, mediaAsset, mediaPath, sequenceEnd } from "./facts.ts";
import { revealLabel } from "./shared.ts";

type Placement = "playhead" | "end";

/** The track a card of this kind goes on: the selected one that fits, else the first free one. */
function trackFor(kind: string): string | null {
  const found = activeSequence();
  if (found === null) return null;
  const want = kind === "audio" ? "audio" : "video";
  const fits = (id: string): boolean =>
    found.sequence.tracks[id as keyof typeof found.sequence.tracks]?.kind === want;
  const selected = viewStore.getState().selection.find((r) => r.kind === "track" && fits(r.id));
  if (selected !== undefined) return selected.id;
  return (
    found.sequence.track_order.find(
      (id) => fits(id) && found.sequence.tracks[id]?.locked !== true,
    ) ?? null
  );
}

/** `timeline.insert` for the asset: at the playhead, or after the last clip of the sequence. */
export function insertArgsFor(id: string | undefined, at: Placement) {
  const found = activeSequence();
  const asset = id === undefined ? null : mediaAsset(id);
  const trackId = asset === null ? null : trackFor(asset.kind);
  const track = trackId === null ? undefined : found?.sequence.tracks[trackId as never];
  if (found === null || id === undefined || trackId === null || track === undefined) {
    return undefined;
  }
  const rate = found.sequence.format.frame_rate;
  const when =
    at === "playhead"
      ? snapToFrame(viewStore.getState().playhead, { num: rate.num, den: rate.den })
      : sequenceEnd(found.sequence);
  // A picture with a "Duration…" choice goes in with that length; else the engine decides.
  const duration = insertDurationFor(docStore.getState().project, id);
  return {
    sequence: found.id,
    track: trackId,
    media: id,
    at: when,
    ...(duration === undefined ? {} : { duration }),
    // Settings > Editing > Keep video and audio linked (WP-2A.13).
    ...(prefsStore.getState().values["editing.link_av"] ? {} : { link: false }),
    overlap: "new_track",
  };
}

const insertArgs = (ctx: MenuContext, at: Placement) => insertArgsFor(ctx.ids[0], at);

/** `timeline.overwrite` of the asset at the playhead (WP-2A.14): what is there is cut out. */
function overwriteArgs(ctx: MenuContext) {
  const args = insertArgs(ctx, "playhead");
  if (args === undefined) return undefined;
  const { overlap: _overlap, ...rest } = args;
  return rest;
}

const canInsert = (ctx: MenuContext): boolean => {
  const id = ctx.ids[0];
  const asset = id === undefined ? null : mediaAsset(id);
  return (
    ctx.ids.length === 1 &&
    asset !== null &&
    asset.kind !== "sequence_ref" &&
    insertArgs(ctx, "playhead") !== undefined
  );
};

const insertReason = (ctx: MenuContext) =>
  ctx.ids.length === 1 ? "contextMenu.reason.noTrack" : "contextMenu.reason.oneAtATime";

/** Whether the asset plays on its own in the Source view: sound or video with a length. */
export function playableMedia(id: string): boolean {
  const card = selectMediaCards(docStore.getState().project).find((c) => c.id === id);
  return (
    card !== undefined &&
    (isMovingPicture(card.kind) || card.kind === "audio") &&
    !card.broken &&
    card.duration !== null &&
    card.duration > 0
  );
}

const playable = (ctx: MenuContext): boolean =>
  ctx.ids.length === 1 && playableMedia(ctx.ids[0] ?? "");

const hasFile = (ctx: MenuContext): boolean =>
  ctx.ids.length === 1 && mediaPath(ctx.ids[0] ?? "") !== null;
const fileReason = (ctx: MenuContext) =>
  ctx.ids.length === 1 ? "contextMenu.reason.noFile" : "contextMenu.reason.oneAtATime";

/** Replace selected clip: one card, and a clip picked on the timeline that it may replace. */
const canReplaceClip = (ctx: MenuContext): boolean => {
  const clip = pickedClip();
  return (
    ctx.ids.length === 1 &&
    canReplaceWith(clip, ctx.ids[0] ?? null) &&
    clipFact(clip ?? "")?.track.locked !== true
  );
};

const replaceClipReason = (ctx: MenuContext) => {
  const clip = pickedClip();
  return ctx.ids.length !== 1
    ? ("contextMenu.reason.oneAtATime" as const)
    : clip === null
      ? ("contextMenu.reason.noPickedClip" as const)
      : clipFact(clip)?.track.locked === true
        ? ("contextMenu.reason.trackLocked" as const)
        : ("contextMenu.reason.cannotReplaceWith" as const);
};

/** Proxies (WP-2F.03b): some target can have one on this engine; some target has one. */
const anyProxyable = (ctx: MenuContext): boolean => {
  const known = proxyStore.getState().byMedia;
  return ctx.ids.some((id) => canHaveProxy(known.get(id)));
};
const anyProxy = (ctx: MenuContext): boolean => {
  const known = proxyStore.getState().byMedia;
  return ctx.ids.some((id) => hasProxy(known.get(id)));
};
/** Video and image sequences have proxies; a selection of several kinds may hold some. */
const PROXY_KINDS =
  "selection.kind == video || selection.kind == image_sequence || selection.kind == mixed";

/** Proxy ▸ Auto / Always / Never: the words for an asset's own proxy mode (ADR-0027). */
const PROXY_MODES: readonly { readonly mode: ProxyMode; readonly label: LabelSpec }[] = [
  { mode: "auto", label: { key: "contextMenu.item.proxyAuto" } },
  { mode: "force", label: { key: "contextMenu.item.proxyAlways" } },
  { mode: "off", label: { key: "contextMenu.item.proxyNever" } },
];

/** The assets of the target that have a proxy mode: video and image sequences. */
const proxyModeTargets = (ctx: MenuContext): string[] =>
  ctx.ids.filter((id) => {
    const kind = mediaAsset(id)?.kind;
    return kind === "video" || kind === "image_sequence";
  });

const proxyModeRows: readonly MenuRow[] = PROXY_MODES.map(
  ({ mode, label }, order): MenuRow => ({
    id: `proxy-mode-${mode}`,
    command: "media.set_proxy_mode",
    group: "file",
    order,
    radio: true,
    label,
    when: "engine.live",
    enabledIf: (ctx) => proxyModeTargets(ctx).length > 0,
    reason: (ctx) =>
      proxyModeTargets(ctx).length > 0
        ? "contextMenu.reason.noEngine"
        : "contextMenu.reason.noProxyMedia",
    // Checked when every asset the row would change has this mode (none: a mixed selection).
    checked: (ctx) => {
      const ids = proxyModeTargets(ctx);
      return ids.length > 0 && ids.every((id) => (mediaAsset(id)?.proxy_mode ?? "auto") === mode);
    },
    args: (ctx) => ({ media: proxyModeTargets(ctx), mode }),
  }),
);

/** Set rating ▸ None, 1 to 5 stars (WP-2F.06b): `media.set_rating` for every card of the target. */
const RATING_CHOICES: readonly (number | null)[] = [null, ...RATINGS];

const ratingRows: readonly MenuRow[] = RATING_CHOICES.map(
  (rating, order): MenuRow => ({
    id: rating === null ? "rating-none" : `rating-${rating}`,
    command: "media.set_rating",
    group: "organize",
    order,
    radio: true,
    label:
      rating === null
        ? { key: "contextMenu.item.ratingNone" }
        : rating === 1
          ? { key: "mediaBin.rating.starOne" }
          : { key: "mediaBin.rating.starMany", vars: { count: rating } },
    when: "engine.live",
    enabledIf: (ctx) => ctx.ids.length <= MAX_TAG_MEDIA,
    reason: (ctx) =>
      ctx.ids.length <= MAX_TAG_MEDIA
        ? "contextMenu.reason.noEngine"
        : "contextMenu.reason.tooManyItems",
    // Checked when every card has this rating (none: the ratings differ).
    checked: (ctx) =>
      ctx.ids.length > 0 && ctx.ids.every((id) => (mediaAsset(id)?.rating ?? null) === rating),
    args: (ctx) => ({ media: ctx.ids, rating }),
  }),
);

/**
 * Add to collection ▸ (WP-2F.06c): one row per manual collection, in name order, each one
 * `media.add_to_collection` for the cards of the target (through `bin.add_to_collection`, which
 * says how many were new). A collection that holds them all already is shown disabled.
 */
function collectionRows(): readonly MenuRow[] {
  const project = docStore.getState().project;
  return selectCollections(project, selectMediaCards(project), selectMediaUsage(project))
    .filter((entry) => entry.kind === "manual")
    .map(
      (entry, index): MenuRow => ({
        id: `collection-${entry.id}`,
        command: "bin.add_to_collection",
        group: "organize",
        order: 1 + index,
        label: { key: "mediaBin.collections.named", vars: { name: entry.name } },
        when: "engine.live",
        enabledIf: (ctx) =>
          ctx.ids.length <= MAX_COLLECTION_MEMBERS &&
          notYetOn(docStore.getState().project, entry.id, ctx.ids).length > 0,
        reason: (ctx) =>
          ctx.keys["engine.live"] === false
            ? "contextMenu.reason.noEngine"
            : "mediaBin.collections.reasonAlreadyIn",
        args: (ctx) => ({ collection: entry.id, media: ctx.ids }),
      }),
    );
}

/** The manual collection the bin has open, when it has one: what "Remove from collection" acts on. */
const openManualCollection = (): string | null => {
  const open = currentOpenCollection();
  return isManualCollection(docStore.getState().project, open) ? open : null;
};

/** What a card menu knows about its target. */
export function binCardKeys(target: { ids: readonly string[] }): ContextValues {
  const project = docStore.getState().project;
  const usage = selectMediaUsage(project);
  const cards = selectMediaCards(project).filter((c) => target.ids.includes(c.id));
  const kinds = new Set(cards.map((c) => c.kind));
  const missing = healthStore.getState().missing;
  return {
    "media.used": cards.some((c) => (usage.get(c.id) ?? 0) > 0),
    "media.offline": cards.some((c) => missing.has(c.id)),
    "selection.kind": kinds.size === 1 ? [...kinds][0] : "mixed",
    "selection.videoCount": cards.filter(
      (c) => c.kind === "video" || c.kind === "image" || c.kind === "image_sequence",
    ).length,
  };
}

/** "Place as" rows (WP-2C.08): one per layout, shown for the number of pictures selected. */
const placeAsRows: readonly MenuRow[] = LAYOUTS.flatMap((layout, order) => {
  const name = LAYOUT_NAMES[layout.id];
  return name === undefined
    ? []
    : [
        {
          id: `place-${layout.id}`,
          command: "bin.place_layout",
          group: "place",
          order,
          visible: `selection.videoCount == ${layout.count}`,
          label: { key: name },
          enabledIf: (ctx: MenuContext) => placeable(ctx.ids),
          reason: "contextMenu.reason.layoutAssets" as const,
          args: (ctx: MenuContext) => ({ media: ctx.ids, layout: layout.id }),
        } satisfies MenuRow,
      ];
});

export const BIN_CARD: MenuSurfaceDef = {
  id: "bin.card",
  name: (target) =>
    target.ids.length > 1
      ? { key: "contextMenu.name.binCards", vars: { count: target.ids.length } }
      : { key: "contextMenu.name.binCard", vars: { name: target.name ?? "" } },
  groups: ["place", "play", "file", "organize", "remove"],
  selection: mediaSelection,
  keys: binCardKeys,
  rows: [
    {
      command: "timeline.insert",
      group: "place",
      order: 0,
      enabledIf: canInsert,
      reason: insertReason,
      args: (ctx) => insertArgs(ctx, "playhead"),
    },
    {
      command: "timeline.overwrite",
      group: "place",
      order: 0.5,
      enabledIf: canInsert,
      reason: insertReason,
      args: overwriteArgs,
    },
    {
      id: "append",
      command: "timeline.insert",
      group: "place",
      order: 1,
      label: { key: "contextMenu.item.appendToTimeline" },
      noHint: true,
      enabledIf: canInsert,
      reason: insertReason,
      args: (ctx) => insertArgs(ctx, "end"),
    },
    {
      command: "bin.replace_clip",
      group: "place",
      order: 1.5,
      enabledIf: canReplaceClip,
      reason: replaceClipReason,
      args: (ctx) => ({ media: ctx.ids[0] }),
    },
    {
      id: "place-layout",
      command: "bin.place_layout",
      group: "place",
      order: 2,
      visible: "selection.count >= 2 && selection.videoCount >= 1",
      label: { key: "contextMenu.item.placeSideBySide" },
      enabledIf: (ctx) => placeable(ctx.ids),
      reason: "contextMenu.reason.layoutAssets",
      args: (ctx) => ({ media: ctx.ids }),
    },
    {
      id: "place-as",
      group: "place",
      order: 3,
      submenu: { label: { key: "contextMenu.item.placeAs" }, rows: placeAsRows },
    },
    {
      command: "bin.insert_duration",
      group: "place",
      order: 4,
      visible: "selection.kind == image",
      when: "selection.count == 1",
      reason: "contextMenu.reason.oneImage",
      args: (ctx) => ({ media: ctx.ids[0] }),
    },
    {
      command: "bin.play_in_source",
      group: "play",
      order: 0,
      enabledIf: playable,
      reason: (ctx) =>
        ctx.ids.length === 1 ? "contextMenu.reason.notPlayable" : "contextMenu.reason.oneAtATime",
      args: (ctx) => ({ id: ctx.ids[0] as MediaId }),
    },
    {
      id: "reveal",
      command: "host.reveal_path",
      group: "file",
      order: 0,
      label: revealLabel,
      enabledIf: hasFile,
      reason: fileReason,
      args: (ctx) => ({ path: mediaPath(ctx.ids[0] ?? "") }),
    },
    {
      command: "bin.relink",
      group: "file",
      order: -1,
      visible: "media.offline",
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
    },
    {
      command: "bin.regenerate_proxies",
      group: "file",
      order: 2,
      visible: PROXY_KINDS,
      when: "engine.live",
      enabledIf: anyProxyable,
      reason: (ctx) =>
        anyProxyable(ctx) ? "contextMenu.reason.noEngine" : "contextMenu.reason.noProxyMedia",
      args: (ctx) => ({ media: ctx.ids }),
    },
    {
      command: "bin.delete_proxies",
      group: "file",
      order: 3,
      visible: PROXY_KINDS,
      when: "engine.live",
      enabledIf: anyProxy,
      reason: (ctx) =>
        anyProxy(ctx) ? "contextMenu.reason.noEngine" : "contextMenu.reason.noProxyToDelete",
      args: (ctx) => ({ media: ctx.ids }),
    },
    {
      id: "proxy-mode",
      group: "file",
      order: 4,
      visible: PROXY_KINDS,
      submenu: { label: { key: "contextMenu.item.proxy" }, rows: proxyModeRows },
    },
    {
      id: "copy-path",
      command: "host.copy_text",
      group: "file",
      order: 1,
      label: { key: "contextMenu.item.copyPath" },
      enabledIf: hasFile,
      reason: fileReason,
      args: (ctx) => ({ text: mediaPath(ctx.ids[0] ?? "") ?? "" }),
    },
    {
      command: "bin.rename",
      group: "organize",
      order: 0,
      when: "selection.count == 1",
      reason: "contextMenu.reason.oneAtATime",
      args: (ctx) => ({ media: ctx.ids[0] }),
    },
    {
      command: "bin.move_to",
      group: "organize",
      order: 1,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
      args: (ctx) => ({ media: ctx.ids }),
    },
    {
      command: "bin.add_tag",
      group: "organize",
      order: 2,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
      args: (ctx) => ({ media: ctx.ids }),
    },
    {
      id: "set-rating",
      group: "organize",
      order: 3,
      submenu: { label: { key: "contextMenu.item.setRating" }, rows: ratingRows },
    },
    {
      id: "add-to-collection",
      group: "organize",
      order: 4,
      submenu: {
        label: { key: "contextMenu.item.addToCollection" },
        rows: [
          {
            id: "new-collection",
            command: "bin.new_collection",
            group: "organize",
            order: 0,
            when: "engine.live",
            reason: "contextMenu.reason.noEngine",
            args: (ctx) => ({ media: ctx.ids }),
          },
        ],
        more: collectionRows,
      },
    },
    {
      // Only while a manual collection is open: the cards leave the list, not the project.
      command: "media.remove_from_collection",
      group: "organize",
      order: 5,
      visibleIf: () => openManualCollection() !== null,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
      args: (ctx) => ({ collection: openManualCollection(), media: ctx.ids }),
    },
    {
      command: "bin.remove_media",
      group: "remove",
      order: 0,
      visible: "selection.count < 2",
      when: "selection.count == 1",
      reason: "contextMenu.reason.oneAtATime",
      args: (ctx) => ({ id: ctx.ids[0] }),
    },
    {
      id: "remove-many",
      command: "bin.remove_selected",
      group: "remove",
      order: 1,
      visible: "selection.count > 1",
      label: (ctx) => ({ key: "contextMenu.item.removeMany", vars: { count: ctx.ids.length } }),
      args: (ctx) => ({ ids: ctx.ids }),
    },
  ],
};

/** A folder tile (one or several selected): open, add inside, rename, move and delete. */
export const BIN_FOLDER: MenuSurfaceDef = {
  id: "bin.folder",
  name: (target) =>
    target.ids.length > 1
      ? { key: "contextMenu.name.binFolders", vars: { count: target.ids.length } }
      : { key: "contextMenu.name.binFolder", vars: { name: target.name ?? "" } },
  groups: ["open", "organize", "remove"],
  selection: folderSelection,
  rows: [
    {
      command: "bin.open_folder",
      group: "open",
      order: 0,
      when: "selection.count == 1",
      reason: "contextMenu.reason.oneAtATime",
      args: (ctx) => ({ folder: ctx.ids[0] }),
    },
    {
      id: "new-inside",
      command: "bin.new_folder",
      group: "organize",
      order: 0,
      when: "selection.count == 1 && engine.live",
      reason: "contextMenu.reason.oneAtATime",
      args: (ctx) => ({ parent: ctx.ids[0] }),
    },
    {
      command: "bin.rename",
      group: "organize",
      order: 1,
      when: "selection.count == 1",
      reason: "contextMenu.reason.oneAtATime",
      args: (ctx) => ({ folder: ctx.ids[0] }),
    },
    {
      command: "bin.move_to",
      group: "organize",
      order: 2,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
      args: (ctx) => ({ folders: ctx.ids }),
    },
    {
      command: "bin.delete_folders",
      group: "remove",
      order: 0,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
      label: (ctx) =>
        ctx.ids.length > 1
          ? { key: "contextMenu.item.deleteFolders", vars: { count: ctx.ids.length } }
          : { key: "contextMenu.item.deleteFolder" },
      args: (ctx) => ({ folders: ctx.ids }),
    },
  ],
};

/** Why "Save as smart collection…" and "Update from current filters" cannot run now, if so. */
const smartReason = (ctx: MenuContext) =>
  ctx.keys["engine.live"] === false
    ? ("contextMenu.reason.noEngine" as const)
    : (currentSmartFilter().reason ?? ("contextMenu.reason.noEngine" as const));

const isSmart = (ctx: MenuContext): boolean => {
  const id = ctx.ids[0];
  const kind =
    id === undefined ? undefined : docStore.getState().project?.collections?.[id as never]?.kind;
  return kind?.type === "smart";
};

/**
 * A collection row of the bin's sidebar (WP-2F.06c): open, rename, update a smart collection from
 * the bin's search and filters, delete. Collections are not selected, so the menu acts on the
 * row it was opened on.
 */
export const BIN_COLLECTION: MenuSurfaceDef = {
  id: "bin.collection",
  name: (target) => ({ key: "contextMenu.name.binCollection", vars: { name: target.name ?? "" } }),
  groups: ["open", "organize", "remove"],
  rows: [
    {
      command: "bin.open_collection",
      group: "open",
      order: 0,
      args: (ctx) => ({ collection: ctx.ids[0] }),
    },
    {
      command: "bin.rename_collection",
      group: "organize",
      order: 0,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
      args: (ctx) => ({ collection: ctx.ids[0] }),
    },
    {
      command: "bin.update_collection_filter",
      group: "organize",
      order: 1,
      visibleIf: isSmart,
      when: "engine.live",
      enabledIf: () => currentSmartFilter().filter !== null,
      reason: smartReason,
      args: (ctx) => ({ collection: ctx.ids[0] }),
    },
    {
      command: "bin.delete_collection",
      group: "remove",
      order: 0,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
      args: (ctx) => ({ collection: ctx.ids[0] }),
    },
  ],
};

const SORT_LABEL: Readonly<Record<string, LabelSpec>> = {
  "imported-desc": { key: "mediaBin.sort.importedDesc" },
  "imported-asc": { key: "mediaBin.sort.importedAsc" },
  "name-asc": { key: "mediaBin.sort.nameAsc" },
  "name-desc": { key: "mediaBin.sort.nameDesc" },
  "duration-asc": { key: "mediaBin.sort.durationAsc" },
  "duration-desc": { key: "mediaBin.sort.durationDesc" },
  "kind-asc": { key: "mediaBin.sort.kindAsc" },
};

const sortRows: readonly MenuRow[] = SORT_CHOICES.map(
  (choice, order): MenuRow => ({
    id: `sort-${choice.by}-${choice.order}`,
    command: "bin.set_sort",
    group: "view",
    order,
    radio: true,
    label: SORT_LABEL[`${choice.by}-${choice.order}`] ?? { key: "mediaBin.sort.button" },
    checked: () => {
      const sort = binStore.getState().sort;
      return sort.by === choice.by && sort.order === choice.order;
    },
    args: () => ({ by: choice.by, order: choice.order }),
  }),
);

const layoutRow = (layout: "grid" | "list", order: number): MenuRow => ({
  id: `layout-${layout}`,
  command: "bin.set_layout",
  group: "view",
  order,
  radio: true,
  label: { key: layout === "grid" ? "mediaBin.layout.grid" : "mediaBin.layout.list" },
  checked: () => layoutStore.getState().layout === layout,
  args: () => ({ layout }),
});

export const BIN_BACKGROUND: MenuSurfaceDef = {
  id: "bin.background",
  name: () => ({ key: "contextMenu.name.binBackground" }),
  groups: ["add", "collections", "select", "view"],
  selection: mediaSelection,
  rows: [
    {
      command: "bin.import",
      group: "add",
      order: 0,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
    },
    {
      command: "bin.import_folder",
      group: "add",
      order: 1,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
    },
    {
      command: "bin.new_folder",
      group: "add",
      order: 2,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
      args: () => ({}),
    },
    {
      command: "bin.import_slideshow",
      group: "add",
      order: 3,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
    },
    {
      command: "bin.new_collection",
      group: "collections",
      order: 0,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
      args: () => ({}),
    },
    {
      // While a search or a filter is on: it becomes a collection that stays up to date.
      command: "bin.save_smart_collection",
      group: "collections",
      order: 1,
      visibleIf: () => currentSmartFilter().filter !== null,
      when: "engine.live",
      reason: "contextMenu.reason.noEngine",
    },
    { command: "bin.select_all", group: "select", order: 0 },
    {
      id: "sort",
      group: "view",
      order: 0,
      submenu: { label: { key: "contextMenu.item.sortBy" }, rows: sortRows },
    },
    {
      id: "layout",
      group: "view",
      order: 1,
      submenu: {
        label: { key: "contextMenu.item.viewAs" },
        rows: [layoutRow("grid", 0), layoutRow("list", 1)],
      },
    },
    {
      command: "bin.toggle_hover_scrub",
      group: "view",
      order: 2,
      checked: () => prefsStore.getState().values["media.hover_scrub"] === true,
    },
  ],
};

export const BIN_CHIP: MenuSurfaceDef = {
  id: "bin.chip",
  name: (target) => ({ key: "contextMenu.name.binChip", vars: { name: target.name ?? "" } }),
  groups: ["filters"],
  rows: [
    {
      command: "bin.remove_filter",
      group: "filters",
      order: 0,
      args: (ctx) => ({ chip: ctx.ids[0] }),
    },
    { command: "bin.clear_filters", group: "filters", order: 1 },
  ],
};
