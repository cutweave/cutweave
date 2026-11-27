// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2F.06c: the collections and the bin sidebar as pure functions and stores: the list the
// sidebar shows, an open collection's members and their order, the filter "Save as smart
// collection" stores, what may be dropped on a collection, the folder tree's rows and keys, and
// the editor state that comes back on the next launch (sidebar shown, open collection).
import type { CollectionId, FolderId, MediaId, Project } from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import { describe, expect, it } from "vitest";
import type { StateStorage } from "zustand/middleware";
import { createBinStore, NO_FILTERS } from "./binStore.ts";
import { computeBinView } from "./binView.ts";
import { selectMediaCards, selectMediaUsage } from "./cards.ts";
import {
  collectionCards,
  collectionNameProblem,
  freeCollectionName,
  isManualCollection,
  notYetOn,
  resolveOpenCollection,
  selectCollections,
  smartFilterOf,
} from "./collections.ts";
import {
  canDropOnCollection,
  createSidebarUiStore,
  folderRows,
  SIDEBAR_MIN_PANEL_WIDTH,
  sidebarMode,
  treeMove,
} from "./sidebar.ts";

const f = (n: number) => `fld_${n}` as FolderId;
const m = (n: number) => `med_${n}` as MediaId;
const c = (n: number) => `col_${n}` as CollectionId;

/**
 * Interviews (1) > Day 1 (2); B-roll (3). Assets 1 and 2 in Day 1, 3 in Interviews, 4 and 5 in
 * B-roll, 6 at the top level; 1, 3 and 4 are tagged "keep". Assets 2 and 4 are on a timeline.
 * Collections: Picks (manual: 5, 1, a member that is gone, 3), Keepers (smart: tag keep),
 * archive (manual, empty), In Interviews (smart: tag keep inside Interviews).
 */
function project(): Project {
  const asset = (n: number, folder: FolderId | null, tags: string[] = []) => ({
    id: m(n),
    kind: "video",
    path_abs: `/m/clip${n}.mp4`,
    path_rel: null,
    name: null,
    folder,
    tags,
    probe: {
      streams: [{ index: 0, kind: "video", codec: null }],
      duration: n * FLICKS_PER_SECOND,
      fps: null,
    },
  });
  const folder = (id: FolderId, name: string, parent: FolderId | null) => ({
    id,
    name,
    parent,
    custom: {},
  });
  return {
    media: {
      [m(1)]: asset(1, f(2), ["keep"]),
      [m(2)]: asset(2, f(2)),
      [m(3)]: asset(3, f(1), ["keep"]),
      [m(4)]: asset(4, f(3), ["keep"]),
      [m(5)]: asset(5, f(3)),
      [m(6)]: asset(6, null),
    },
    folders: {
      [f(1)]: folder(f(1), "Interviews", null),
      [f(2)]: folder(f(2), "Day 1", f(1)),
      [f(3)]: folder(f(3), "B-roll", null),
    },
    collections: {
      [c(1)]: {
        id: c(1),
        name: "Picks",
        kind: { type: "manual", media: [m(5), m(1), "med_gone", m(3)] },
        custom: {},
      },
      [c(2)]: {
        id: c(2),
        name: "Keepers",
        kind: { type: "smart", filter: { tags: ["keep"] } },
        custom: {},
      },
      [c(3)]: { id: c(3), name: "archive", kind: { type: "manual" }, custom: {} },
      [c(4)]: {
        id: c(4),
        name: "In Interviews",
        kind: { type: "smart", filter: { tags: ["keep"], folder: f(1), recursive: true } },
        custom: {},
      },
    },
    sequences: {
      seq_1: { tracks: { trk_1: { items: { itm_1: { media: m(2) }, itm_2: { media: m(4) } } } } },
    },
  } as unknown as Project;
}

const facts = (p: Project) => ({ cards: selectMediaCards(p), usage: selectMediaUsage(p) });
const ids = (cards: readonly { id: string }[]) => cards.map((card) => card.id);
const input = {
  text: "",
  filters: NO_FILTERS,
  sort: { by: "name", order: "asc" },
  scope: "all",
} as const;

describe("the collections the sidebar lists", () => {
  it("lists them by name ignoring case, with the kind and the number of members now", () => {
    const p = project();
    const { cards, usage } = facts(p);
    expect(selectCollections(p, cards, usage).map((e) => [e.name, e.kind, e.count])).toEqual([
      ["archive", "manual", 0],
      ["In Interviews", "smart", 2],
      ["Keepers", "smart", 3],
      // The member that names no asset is not counted.
      ["Picks", "manual", 3],
    ]);
  });

  it("gives a smart collection its filter and a manual one none", () => {
    const p = project();
    const { cards, usage } = facts(p);
    const list = selectCollections(p, cards, usage);
    expect(list.find((e) => e.id === c(2))?.filter).toEqual({ tags: ["keep"] });
    expect(list.find((e) => e.id === c(1))?.filter).toBeNull();
  });

  it("has none for a project without collections, or no project", () => {
    const p = project();
    delete (p as { collections?: unknown }).collections;
    expect(selectCollections(p, selectMediaCards(p), new Map())).toEqual([]);
    expect(selectCollections(null, [], new Map())).toEqual([]);
  });

  it("counts nothing for a smart collection whose folder was deleted", () => {
    const p = project();
    delete (p.folders as Record<string, unknown>)[f(1)];
    delete (p.folders as Record<string, unknown>)[f(2)];
    const { cards, usage } = facts(p);
    expect(selectCollections(p, cards, usage).find((e) => e.id === c(4))?.count).toBe(0);
  });
});

describe("an open collection", () => {
  it("shows a manual collection's members in their stored order, whatever the sort", () => {
    const p = project();
    const { cards, usage } = facts(p);
    for (const order of ["asc", "desc"] as const) {
      expect(ids(collectionCards(p, cards, usage, c(1), {}, { by: "name", order }))).toEqual([
        m(5),
        m(1),
        m(3),
      ]);
    }
  });

  it("shows a smart collection's live members in the bin's sort order", () => {
    const p = project();
    const { cards, usage } = facts(p);
    expect(ids(collectionCards(p, cards, usage, c(2), {}, { by: "name", order: "desc" }))).toEqual([
      m(4),
      m(3),
      m(1),
    ]);
    // An asset that gets the tag joins it: nothing is stored.
    (p.media[m(6)] as { tags?: string[] }).tags = ["keep"];
    const next = { ...p };
    expect(
      ids(
        collectionCards(
          next,
          selectMediaCards(next),
          usage,
          c(2),
          {},
          { by: "name", order: "asc" },
        ),
      ),
    ).toEqual([m(1), m(3), m(4), m(6)]);
  });

  it("narrows the members by the bin's search and filters", () => {
    const p = project();
    const { cards, usage } = facts(p);
    expect(ids(collectionCards(p, cards, usage, c(1), { text: "clip5" }, null))).toEqual([m(5)]);
    expect(ids(collectionCards(p, cards, usage, c(2), { used: true }, null))).toEqual([m(4)]);
  });

  it("is what the bin view shows: flat, without folder tiles, across folders", () => {
    const p = project();
    const view = computeBinView(p, input, null, c(1));
    expect(view.collection).toBe(c(1));
    expect(view.open).toBeNull();
    expect(view.folders).toEqual([]);
    expect(ids(view.cards)).toEqual([m(5), m(1), m(3)]);
    expect(view.order).toEqual([m(5), m(1), m(3)].map((id) => ({ kind: "media", id })));
  });

  it("shows nothing for a collection that does not exist", () => {
    const p = project();
    const { cards, usage } = facts(p);
    expect(collectionCards(p, cards, usage, c(9), {}, null)).toEqual([]);
    expect(resolveOpenCollection(p, c(9))).toBeNull();
    expect(resolveOpenCollection(p, c(1))).toBe(c(1));
    expect(resolveOpenCollection(null, c(1))).toBeNull();
  });
});

describe("naming a collection", () => {
  it("proposes a name no collection has, ignoring case", () => {
    const p = project();
    expect(freeCollectionName(p, "New collection")).toBe("New collection");
    expect(freeCollectionName(p, "picks")).toBe("picks 2");
    (p.collections as Record<string, unknown>)[c(7)] = {
      id: c(7),
      name: "Picks 2",
      kind: { type: "manual" },
      custom: {},
    };
    expect(freeCollectionName(p, "Picks")).toBe("Picks 3");
  });

  it("says why a name cannot be stored", () => {
    const p = project();
    expect(collectionNameProblem(p, "  ")).toBe("empty");
    expect(collectionNameProblem(p, "x".repeat(256))).toBe("tooLong");
    expect(collectionNameProblem(p, "a\u0007b")).toBe("control");
    expect(collectionNameProblem(p, " PICKS ")).toBe("taken");
    // Its own name, in another case, is a rename and not a clash.
    expect(collectionNameProblem(p, "picks", c(1))).toBeNull();
    expect(collectionNameProblem(p, "Selects")).toBeNull();
  });
});

describe("Save as smart collection", () => {
  const bin = (over: object = {}) =>
    ({ text: "", filters: NO_FILTERS, scope: "all", ...over }) as never;

  it("stores the bin's search and filters as the engine's filter", () => {
    expect(
      smartFilterOf(
        bin({
          text: "beach",
          filters: { ...NO_FILTERS, kinds: ["video"], use: "unused", tags: ["keep"], minRating: 4 },
        }),
        null,
      ),
    ).toEqual({ text: "beach", kinds: ["video"], used: false, tags: ["keep"], min_rating: 4 });
  });

  it("includes the open folder and its subfolders when the search is scoped to This folder", () => {
    const filters = { ...NO_FILTERS, tags: ["keep"] };
    expect(smartFilterOf(bin({ filters, scope: "folder" }), f(1))).toEqual({
      tags: ["keep"],
      folder: f(1),
      recursive: true,
    });
    // All media, or the top level: no folder.
    expect(smartFilterOf(bin({ filters, scope: "all" }), f(1))).toEqual({ tags: ["keep"] });
    expect(smartFilterOf(bin({ filters, scope: "folder" }), null)).toEqual({ tags: ["keep"] });
  });

  it("has nothing to store while no search or filter is on", () => {
    expect(smartFilterOf(bin(), f(1))).toBeNull();
    expect(smartFilterOf(bin({ text: "   ", scope: "folder" }), f(1))).toBeNull();
  });

  it("gives the members the saved collection will have", () => {
    const p = project();
    const filter = smartFilterOf(
      bin({ filters: { ...NO_FILTERS, tags: ["keep"] }, scope: "folder" }),
      f(1),
    );
    (p.collections as Record<string, unknown>)[c(8)] = {
      id: c(8),
      name: "Saved",
      kind: { type: "smart", filter },
      custom: {},
    };
    const { cards, usage } = facts(p);
    expect(ids(collectionCards(p, cards, usage, c(8), {}, { by: "name", order: "asc" }))).toEqual([
      m(1),
      m(3),
    ]);
  });
});

describe("dropping cards onto a collection", () => {
  it("adds only what a manual collection does not hold yet", () => {
    const p = project();
    expect(notYetOn(p, c(1), [m(1), m(2), m(6)])).toEqual([m(2), m(6)]);
    expect(notYetOn(p, c(3), [m(1)])).toEqual([m(1)]);
    // An asset that is not in the project is not added.
    expect(notYetOn(p, c(3), ["med_gone"])).toEqual([]);
  });

  it("accepts a drop that would add an asset, and refuses the rest", () => {
    const p = project();
    expect(canDropOnCollection(p, { media: [m(2)], folders: [] }, c(1))).toBe(true);
    // Everything dragged is on it already.
    expect(canDropOnCollection(p, { media: [m(1), m(5)], folders: [] }, c(1))).toBe(false);
    // A smart collection's members come from its filter.
    expect(canDropOnCollection(p, { media: [m(2)], folders: [] }, c(2))).toBe(false);
    // Folders are not members.
    expect(canDropOnCollection(p, { media: [], folders: [f(3)] }, c(1))).toBe(false);
    expect(canDropOnCollection(p, { media: [m(2)], folders: [] }, c(9))).toBe(false);
    expect(canDropOnCollection(null, { media: [m(2)], folders: [] }, c(1))).toBe(false);
  });

  it("knows a manual collection from a smart one", () => {
    const p = project();
    expect(isManualCollection(p, c(1))).toBe(true);
    expect(isManualCollection(p, c(2))).toBe(false);
    expect(isManualCollection(p, null)).toBe(false);
  });
});

describe("the folder tree of the sidebar", () => {
  const names = (p: Project, collapsed: string[] = []) =>
    folderRows(p, new Set(collapsed)).map((row) => `${"  ".repeat(row.level - 1)}${row.name}`);

  it("starts with All media, then the folders in name order with their subfolders", () => {
    expect(names(project())).toEqual(["null", "B-roll", "Interviews", "  Day 1"]);
    const rows = folderRows(project(), new Set());
    expect(rows[0]).toMatchObject({ id: null, level: 1, hasChildren: false });
    expect(rows[2]).toMatchObject({ id: f(1), hasChildren: true, expanded: true });
    expect(rows[3]).toMatchObject({ id: f(2), parent: f(1), level: 2, hasChildren: false });
  });

  it("hides the subfolders of a collapsed folder", () => {
    expect(names(project(), [f(1)])).toEqual(["null", "B-roll", "Interviews"]);
    expect(folderRows(project(), new Set([f(1)]))[2]).toMatchObject({ expanded: false });
  });

  it("is only All media for a project without folders", () => {
    expect(folderRows(null, new Set())).toHaveLength(1);
  });

  it("moves with the arrow keys as a tree does", () => {
    const rows = folderRows(project(), new Set());
    expect(treeMove(rows, 0, "ArrowDown")).toEqual({ kind: "focus", index: 1 });
    expect(treeMove(rows, 3, "ArrowDown")).toBeNull();
    expect(treeMove(rows, 0, "ArrowUp")).toBeNull();
    expect(treeMove(rows, 1, "End")).toEqual({ kind: "focus", index: 3 });
    expect(treeMove(rows, 3, "Home")).toEqual({ kind: "focus", index: 0 });
    // Right on an open folder goes to its first child; Left on a child goes to its parent.
    expect(treeMove(rows, 2, "ArrowRight")).toEqual({ kind: "focus", index: 3 });
    expect(treeMove(rows, 3, "ArrowLeft")).toEqual({ kind: "focus", index: 2 });
    expect(treeMove(rows, 2, "ArrowLeft")).toEqual({ kind: "collapse", index: 2 });
    // A folder without subfolders has nothing to open.
    expect(treeMove(rows, 1, "ArrowRight")).toBeNull();
    expect(treeMove(rows, 1, "ArrowLeft")).toBeNull();
    const closed = folderRows(project(), new Set([f(1)]));
    expect(treeMove(closed, 2, "ArrowRight")).toEqual({ kind: "expand", index: 2 });
    expect(treeMove(rows, 1, "a")).toBeNull();
  });
});

describe("the sidebar's state", () => {
  it("is shown beside the grid in a wide panel and over it in a narrow one", () => {
    expect(sidebarMode(SIDEBAR_MIN_PANEL_WIDTH)).toBe("inline");
    expect(sidebarMode(SIDEBAR_MIN_PANEL_WIDTH - 1)).toBe("overlay");
    // Not measured yet: as a wide panel, so nothing jumps on the first paint.
    expect(sidebarMode(0)).toBe("inline");
  });

  it("opens a collection in place of the folder, and a folder in place of the collection", () => {
    const store = createBinStore();
    store.getState().openFolder("p", f(1));
    store.getState().openCollection("p", c(1));
    expect(store.getState().openCollections).toEqual({ p: c(1) });
    expect(store.getState().openFolders).toEqual({});
    store.getState().openFolder("p", f(3));
    expect(store.getState().openCollections).toEqual({});
    expect(store.getState().openFolders).toEqual({ p: f(3) });
    store.getState().openCollection("p", c(2));
    store.getState().openFolder("p", null);
    expect(store.getState().openCollections).toEqual({});
    // Another project's place is its own.
    store.getState().openCollection("q", c(2));
    store.getState().openCollection("p", null);
    expect(store.getState().openCollections).toEqual({ q: c(2) });
  });

  it("keeps whether it is shown and what is open for the next launch, as editor state", () => {
    const data = new Map<string, string>();
    const storage: StateStorage = {
      getItem: (key) => data.get(key) ?? null,
      setItem: (key, value) => void data.set(key, value),
      removeItem: (key) => void data.delete(key),
    };
    const first = createBinStore(storage);
    expect(first.getState().sidebar).toBe(false);
    first.getState().setSidebar(true);
    first.getState().openCollection("C:/p/film.cutweave", c(2));
    const second = createBinStore(storage);
    expect(second.getState().sidebar).toBe(true);
    expect(second.getState().openCollections).toEqual({ "C:/p/film.cutweave": c(2) });
  });

  it("ignores a malformed stored value", () => {
    const storage: StateStorage = {
      getItem: () =>
        JSON.stringify({
          state: { sidebar: "yes", openCollections: { p: 3, q: c(1) } },
          version: 1,
        }),
      setItem: () => undefined,
      removeItem: () => undefined,
    };
    const store = createBinStore(storage);
    expect(store.getState().sidebar).toBe(false);
    expect(store.getState().openCollections).toEqual({ q: c(1) });
  });

  it("remembers collapsed folders, the rename and the drop target for the session", () => {
    const store = createSidebarUiStore();
    store.getState().setCollapsed(f(1), true);
    expect([...store.getState().collapsed]).toEqual([f(1)]);
    store.getState().setCollapsed(f(1), false);
    expect(store.getState().collapsed.size).toBe(0);
    store.getState().startRename(c(1));
    expect(store.getState().renaming).toBe(c(1));
    store.getState().stopRename();
    expect(store.getState().renaming).toBeNull();
    store.getState().setDropOver(c(1));
    expect(store.getState().dropOver).toBe(c(1));
  });
});
