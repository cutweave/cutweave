// SPDX-License-Identifier: GPL-3.0-or-later
// The media bin's menus (WP-2H.13): a card (one or several selected), the background and the empty
// bin, and a filter chip. Rows name commands that exist today; what later work packages add is
// listed in the TODO below, not stubbed.
// TODO(WP-2A.14): Overwrite at playhead. TODO(WP-2B.01): New timeline from selection.
// TODO(WP-2F.06, WP-2F.03): Add tag, Relink, Regenerate proxies. Each work package adds its row
// to these tables. Folders (WP-2F.08): New folder, Rename, Move to, Open, Delete and Select all.
import type { MediaId } from "@cutweave/protocol";
import { snapToFrame } from "@cutweave/timeline";
import { binStore, SORT_CHOICES } from "../../media/binStore.ts";
import { selectMediaCards, selectMediaUsage } from "../../media/cards.ts";
import { layoutStore } from "../../media/layoutStore.ts";
import { prefsStore } from "../../prefs/prefsStore.ts";
import { docStore } from "../../state/docStore.ts";
import { viewStore } from "../../state/viewStore.ts";
import { folderSelection, mediaSelection } from "../adapters.ts";
import type { LabelSpec, MenuContext, MenuRow, MenuSurfaceDef } from "../types.ts";
import type { ContextValues } from "../when.ts";
import { activeSequence, mediaAsset, mediaPath, sequenceEnd } from "./facts.ts";
import { revealLabel } from "./shared.ts";
import { placeable } from "../../layout/actions.ts";
import { LAYOUT_NAMES } from "../../layout/names.ts";
import { LAYOUTS } from "../../layout/table.ts";

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
  return {
    sequence: found.id,
    track: trackId,
    media: id,
    at: when,
    overlap: "new_track",
  };
}

const insertArgs = (ctx: MenuContext, at: Placement) => insertArgsFor(ctx.ids[0], at);

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
    (card.kind === "video" || card.kind === "audio") &&
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

/** What a card menu knows about its target. */
export function binCardKeys(target: { ids: readonly string[] }): ContextValues {
  const project = docStore.getState().project;
  const usage = selectMediaUsage(project);
  const cards = selectMediaCards(project).filter((c) => target.ids.includes(c.id));
  const kinds = new Set(cards.map((c) => c.kind));
  return {
    "media.used": cards.some((c) => (usage.get(c.id) ?? 0) > 0),
    "selection.kind": kinds.size === 1 ? [...kinds][0] : "mixed",
    "selection.videoCount": cards.filter((c) => c.kind === "video" || c.kind === "image").length,
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
      id: "place-layout",
      command: "bin.place_layout",
      group: "place",
      order: 2,
      visible: "selection.videoCount >= 2",
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
  groups: ["add", "select", "view"],
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
