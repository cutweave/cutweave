// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2F.08: the bin's folder rules as pure functions: the selection model (ranges across folders
// and assets), where a drop lands and whether it is allowed, the folder tree and the delete
// dialog's wording for 0, 1 and several used media.
import type { FolderId, MediaId, Project } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { createI18n } from "../i18n/index.ts";
import { translator } from "../timeline/errors.ts";
import { NO_FILTERS } from "./binStore.ts";
import { computeBinView } from "./binView.ts";
import { selectMediaCards, selectMediaUsage } from "./cards.ts";
import {
  breadcrumbOf,
  canDrop,
  childFolders,
  clickTile,
  type DropElement,
  folderPathText,
  NO_TILES,
  planFolderDelete,
  pruneSelection,
  resolveDropTarget,
  resolveOpenFolder,
  selectAllTiles,
  type Tile,
} from "./folders.ts";
import { countText, folderDeleteText } from "./folderText.ts";

const f = (n: number) => `fld_${n}` as FolderId;
const m = (n: number) => `med_${n}` as MediaId;

/**
 * Interviews (1) > Day 1 (2); B-roll (3). Assets: 1 and 2 in Day 1, 3 in Interviews, 4 and 5 in
 * B-roll, 6 at the top level. Assets 2, 4 and 5 are on a timeline.
 */
function project(): Project {
  const asset = (n: number, folder: FolderId | null) => ({
    id: m(n),
    kind: "video",
    path_abs: `/m/clip${n}.mp4`,
    path_rel: null,
    name: null,
    folder,
    probe: { streams: [{ index: 0, kind: "video", codec: null }], duration: 1, fps: null },
  });
  const folder = (id: FolderId, name: string, parent: FolderId | null) => ({
    id,
    name,
    parent,
    custom: {},
  });
  const item = (media: MediaId) => ({ media });
  return {
    media: {
      [m(1)]: asset(1, f(2)),
      [m(2)]: asset(2, f(2)),
      [m(3)]: asset(3, f(1)),
      [m(4)]: asset(4, f(3)),
      [m(5)]: asset(5, f(3)),
      [m(6)]: asset(6, null),
    },
    folders: {
      [f(1)]: folder(f(1), "Interviews", null),
      [f(2)]: folder(f(2), "Day 1", f(1)),
      [f(3)]: folder(f(3), "B-roll", null),
    },
    sequences: {
      seq_1: {
        tracks: {
          trk_1: { items: { itm_1: item(m(2)), itm_2: item(m(4)), itm_3: item(m(5)) } },
        },
      },
    },
  } as unknown as Project;
}

const tile = (kind: Tile["kind"], id: string): Tile => ({ kind, id });
const keys = (tiles: readonly Tile[]) => tiles.map((t) => `${t.kind}:${t.id}`);

describe("the selection model", () => {
  const order: Tile[] = [
    tile("folder", f(1)),
    tile("folder", f(3)),
    tile("media", m(1)),
    tile("media", m(2)),
    tile("media", m(3)),
    tile("media", m(4)),
  ];
  const plain = { ctrl: false, shift: false };

  it("a plain click selects only that tile and anchors there", () => {
    const first = clickTile(order, NO_TILES, tile("media", m(2)), plain);
    expect(keys(first.tiles)).toEqual([`media:${m(2)}`]);
    const second = clickTile(order, first, tile("folder", f(3)), plain);
    expect(keys(second.tiles)).toEqual([`folder:${f(3)}`]);
    expect(second.anchor).toEqual(tile("folder", f(3)));
  });

  it("Ctrl or Cmd toggles a tile and moves the anchor", () => {
    let sel = clickTile(order, NO_TILES, tile("media", m(1)), plain);
    sel = clickTile(order, sel, tile("media", m(3)), { ctrl: true, shift: false });
    sel = clickTile(order, sel, tile("media", m(4)), { ctrl: true, shift: false });
    expect(keys(sel.tiles)).toEqual([`media:${m(1)}`, `media:${m(3)}`, `media:${m(4)}`]);
    sel = clickTile(order, sel, tile("media", m(3)), { ctrl: true, shift: false });
    expect(keys(sel.tiles)).toEqual([`media:${m(1)}`, `media:${m(4)}`]);
    expect(sel.anchor).toEqual(tile("media", m(3)));
  });

  it("Shift extends a range from the anchor, across folders and assets", () => {
    const anchored = clickTile(order, NO_TILES, tile("folder", f(3)), plain);
    const range = clickTile(order, anchored, tile("media", m(2)), { ctrl: false, shift: true });
    expect(keys(range.tiles)).toEqual([`folder:${f(3)}`, `media:${m(1)}`, `media:${m(2)}`]);
    // The anchor stays, so the range follows the next Shift click, in either direction.
    const back = clickTile(order, range, tile("folder", f(1)), { ctrl: false, shift: true });
    expect(keys(back.tiles)).toEqual([`folder:${f(1)}`, `folder:${f(3)}`]);
    expect(back.anchor).toEqual(tile("folder", f(3)));
  });

  it("Ctrl+Shift adds a range to what is selected", () => {
    let sel = clickTile(order, NO_TILES, tile("folder", f(1)), plain);
    sel = clickTile(order, sel, tile("media", m(3)), { ctrl: true, shift: false });
    sel = clickTile(order, sel, tile("media", m(4)), { ctrl: true, shift: true });
    expect(keys(sel.tiles)).toEqual([`folder:${f(1)}`, `media:${m(3)}`, `media:${m(4)}`]);
  });

  it("Shift with no anchor selects just the clicked tile", () => {
    const sel = clickTile(order, NO_TILES, tile("media", m(3)), { ctrl: false, shift: true });
    expect(keys(sel.tiles)).toEqual([`media:${m(3)}`]);
  });

  it("selects all, and forgets tiles that left the grid", () => {
    const all = selectAllTiles(order);
    expect(all.tiles).toHaveLength(6);
    const fewer = pruneSelection(order.slice(0, 3), all);
    expect(fewer.tiles).toHaveLength(3);
    expect(pruneSelection(order, all)).toBe(all);
  });
});

/** An element chain that answers `getAttribute` like the DOM. */
function chain(...attributes: (string | null)[]): DropElement {
  let parent: DropElement | null = null;
  for (const value of [...attributes].reverse()) {
    const up: DropElement | null = parent;
    parent = {
      getAttribute: (name) => (name === "data-drop-folder" ? value : null),
      parentElement: up,
    };
  }
  return parent as DropElement;
}

describe("drop targets", () => {
  it("resolves the closest folder tile or breadcrumb segment", () => {
    // From the tile's name span up to the tile, and on to the grid.
    expect(resolveDropTarget(chain(null, f(2), null))).toEqual({ folder: f(2) });
    expect(resolveDropTarget(chain(f(3), f(2)))).toEqual({ folder: f(3) });
  });

  it("reads an empty marker as the top level and nothing as no target", () => {
    expect(resolveDropTarget(chain(null, ""))).toEqual({ folder: null });
    expect(resolveDropTarget(chain(null, null))).toBeNull();
    expect(resolveDropTarget(null)).toBeNull();
  });

  it("allows a move to a place something is not yet", () => {
    const p = project();
    expect(canDrop(p, { media: [m(6)], folders: [] }, f(1))).toBe(true);
    expect(canDrop(p, { media: [m(1)], folders: [] }, null)).toBe(true);
    expect(canDrop(p, { media: [], folders: [f(3)] }, f(1))).toBe(true);
  });

  it("refuses what changes nothing", () => {
    const p = project();
    expect(canDrop(p, { media: [m(1), m(2)], folders: [] }, f(2))).toBe(false);
    expect(canDrop(p, { media: [m(6)], folders: [] }, null)).toBe(false);
    expect(canDrop(p, { media: [], folders: [f(3)] }, null)).toBe(false);
    // One that moves is enough.
    expect(canDrop(p, { media: [m(1), m(6)], folders: [] }, f(2))).toBe(true);
  });

  it("never drops a folder into itself or below itself", () => {
    const p = project();
    expect(canDrop(p, { media: [], folders: [f(1)] }, f(1))).toBe(false);
    expect(canDrop(p, { media: [], folders: [f(1)] }, f(2))).toBe(false);
    expect(canDrop(p, { media: [m(6)], folders: [f(1)] }, f(2))).toBe(false);
    expect(canDrop(p, { media: [], folders: [f(2)] }, f(1))).toBe(false);
    expect(canDrop(p, { media: [], folders: [f(2)] }, null)).toBe(true);
  });

  it("refuses a folder that does not exist", () => {
    expect(canDrop(project(), { media: [m(6)], folders: [] }, f(99))).toBe(false);
    expect(canDrop(null, { media: [m(6)], folders: [] }, null)).toBe(false);
  });
});

describe("the folder tree", () => {
  it("lists a folder's subfolders by name with their counts", () => {
    const p = project();
    const cards = selectMediaCards(p);
    const top = childFolders(p, cards, null);
    expect(top.map((t) => [t.name, t.media, t.folders, t.items])).toEqual([
      ["B-roll", 2, 0, 2],
      ["Interviews", 1, 1, 2],
    ]);
    expect(childFolders(p, cards, f(1)).map((t) => t.name)).toEqual(["Day 1"]);
  });

  it("builds the breadcrumb and the path text", () => {
    const p = project();
    expect(breadcrumbOf(p, f(2)).map((c) => c.name)).toEqual([null, "Interviews", "Day 1"]);
    expect(breadcrumbOf(p, null)).toEqual([{ id: null, name: null }]);
    expect(folderPathText(p, f(2))).toBe("Interviews / Day 1");
    expect(folderPathText(p, null)).toBe("");
  });

  it("reads a folder that no longer exists as the top level", () => {
    expect(resolveOpenFolder(project(), f(2))).toBe(f(2));
    expect(resolveOpenFolder(project(), f(99))).toBeNull();
    expect(resolveOpenFolder(null, f(2))).toBeNull();
  });
});

describe("what the bin shows", () => {
  const input = {
    text: "",
    filters: NO_FILTERS,
    sort: { by: "name", order: "asc" } as const,
    scope: "all",
  } as const;
  const names = (v: ReturnType<typeof computeBinView>) =>
    v.cards.map((c) => c.id.replace("med_", ""));

  it("browses one level of the open folder", () => {
    const p = project();
    expect(names(computeBinView(p, input, null))).toEqual(["6"]);
    expect(names(computeBinView(p, input, f(1)))).toEqual(["3"]);
    expect(computeBinView(p, input, f(1)).folders.map((t) => t.name)).toEqual(["Day 1"]);
    expect(names(computeBinView(p, input, f(2)))).toEqual(["1", "2"]);
  });

  it("searches All media flat, with matching folders first", () => {
    const p = project();
    const found = computeBinView(p, { ...input, text: "inter" }, f(3));
    expect(found.searching).toBe(true);
    expect(found.folders.map((t) => t.name)).toEqual(["Interviews"]);
    expect(found.cards).toHaveLength(0);
    const clips = computeBinView(p, { ...input, text: "clip" }, null);
    expect(clips.cards).toHaveLength(6);
    expect(clips.folders).toHaveLength(0);
    expect(clips.order[0]?.kind).toBe("media");
  });

  it("limits This folder to the open folder and its subfolders", () => {
    const p = project();
    const here = computeBinView(p, { ...input, text: "clip", scope: "folder" }, f(1));
    expect(names(here)).toEqual(["1", "2", "3"]);
    const day = computeBinView(p, { ...input, text: "day", scope: "folder" }, f(1));
    expect(day.folders.map((t) => t.name)).toEqual(["Day 1"]);
    // At the top level This folder is the whole bin.
    expect(computeBinView(p, { ...input, text: "clip", scope: "folder" }, null).cards).toHaveLength(
      6,
    );
  });

  it("lists folders before assets in the order a Shift range walks", () => {
    const view = computeBinView(project(), input, null);
    expect(view.order.map((t) => t.kind)).toEqual(["folder", "folder", "media"]);
  });
});

describe("the delete dialog", () => {
  const i18n = createI18n("en");
  const t = translator(i18n.t.bind(i18n));
  const plan = (folder: FolderId) => {
    const p = project();
    const cards = selectMediaCards(p);
    return planFolderDelete(p, cards, selectMediaUsage(p), folder);
  };

  it("counts what a folder holds, below it too", () => {
    expect(plan(f(1))).toMatchObject({ media: 3, used: 1, subfolders: 1, empty: false });
    expect(plan(f(3))).toMatchObject({ media: 2, used: 2, subfolders: 0, parent: null });
    expect(plan(f(2))).toMatchObject({ media: 2, used: 1, parentName: "Interviews" });
    expect(plan(f(99))).toBeNull();
  });

  it("an empty folder deletes without asking", () => {
    const p = project();
    delete (p.media as Record<string, unknown>)[m(1)];
    delete (p.media as Record<string, unknown>)[m(2)];
    const cards = selectMediaCards(p);
    const empty = planFolderDelete(p, cards, selectMediaUsage(p), f(2));
    expect(empty).toMatchObject({ media: 0, subfolders: 0, empty: true });
  });

  it("says what it holds and names the parent for used media: several used", () => {
    const text = folderDeleteText(plan(f(3)) as NonNullable<ReturnType<typeof plan>>, t);
    expect(text.title).toBe("Delete “B-roll” and its 2 media?");
    expect(text.notes).toEqual(["2 are used on the timeline and will move to “Media”."]);
  });

  it("says it for one used medium, inside another folder", () => {
    const text = folderDeleteText(plan(f(2)) as NonNullable<ReturnType<typeof plan>>, t);
    expect(text.title).toBe("Delete “Day 1” and its 2 media?");
    expect(text.notes).toEqual(["1 is used on the timeline and will move to “Interviews”."]);
  });

  it("says nothing about the timeline when no medium is used, and mentions subfolders", () => {
    const p = project();
    p.sequences = {} as Project["sequences"];
    const cards = selectMediaCards(p);
    const none = planFolderDelete(p, cards, selectMediaUsage(p), f(1)) as NonNullable<
      ReturnType<typeof plan>
    >;
    expect(none.used).toBe(0);
    const text = folderDeleteText(none, t);
    expect(text.title).toBe("Delete “Interviews” and its 3 media?");
    expect(text.notes).toEqual(["It also holds 1 folder."]);
  });

  it("words one medium, and a folder that holds only folders", () => {
    const one = folderDeleteText(
      {
        folder: f(1),
        name: "A",
        parent: null,
        parentName: null,
        media: 1,
        used: 0,
        subfolders: 0,
        empty: false,
      },
      t,
    );
    expect(one.title).toBe("Delete “A” and its 1 media?");
    expect(one.notes).toEqual([]);
    const onlyFolders = folderDeleteText(
      {
        folder: f(1),
        name: "A",
        parent: null,
        parentName: null,
        media: 0,
        used: 0,
        subfolders: 3,
        empty: false,
      },
      t,
    );
    expect(onlyFolders.title).toBe("Delete “A” and its 3 folders?");
    const oneUsed = folderDeleteText(
      {
        folder: f(1),
        name: "A",
        parent: null,
        parentName: null,
        media: 4,
        used: 1,
        subfolders: 2,
        empty: false,
      },
      t,
    );
    expect(oneUsed.notes).toEqual([
      "It also holds 2 folders.",
      "1 is used on the timeline and will move to “Media”.",
    ]);
  });

  it("counts items and names folder tiles for a screen reader", () => {
    expect(countText(0, t)).toBe("Empty");
    expect(countText(1, t)).toBe("1 item");
    expect(countText(12, t)).toBe("12 items");
  });
});
