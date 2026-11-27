// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The bin's sidebar and collections (WP-2F.06c, ADR-0036): the toggle and where the sidebar shows
// (beside the grid, or over it in a narrow panel), the folder tree with its keys and drop targets,
// the collections (open, new, rename, delete, drop cards onto one), an open collection in the
// grid, Save as smart collection from the search and filters, and the "Used" badge.
import type { CollectionId, FolderId, Project } from "@cutweave/protocol";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { Fragment, createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { binStore, initialBinState } from "../media/binStore.ts";
import { dragStore, TILE_DRAG_TYPE, tileDragStore } from "../media/dragStore.ts";
import { folderUiStore } from "../media/folderUi.ts";
import { mediaStore } from "../media/mediaStore.ts";
import { SIDEBAR_MIN_PANEL_WIDTH, sidebarUiStore } from "../media/sidebar.ts";
import { placeholderThumbnails, setThumbnailSource } from "../media/thumbnails.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { MenuDialogs } from "../menus/MenuDialogs.tsx";
import { paletteAction } from "../palette/actions.ts";
import { registerBuiltinPaletteActions } from "../palette/builtinActions.ts";
import { historyLabel, undoText } from "../session/historyLabels.ts";
import { sessionStore } from "../session/sessionStore.ts";
import { docStore } from "../state/docStore.ts";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { translator } from "../timeline/errors.ts";
import { MediaPanel } from "./MediaPanel.tsx";

const F = 705_600_000;
const PATH = "C:/p/film.cutweave";
const INTERVIEWS = "fld_interviews" as FolderId;
const DAY1 = "fld_day1" as FolderId;
const BROLL = "fld_broll" as FolderId;
const PICKS = "col_picks" as CollectionId;
const KEEPERS = "col_keepers" as CollectionId;
const ARCHIVE = "col_archive" as CollectionId;

const asset = (id: string, name: string, folder: FolderId | null, tags: string[] = []) => ({
  id,
  kind: "video",
  path_abs: `/media/${name}`,
  path_rel: null,
  name: null,
  folder,
  tags,
  probe: {
    streams: [{ index: 0, kind: "video", codec: "h264", width: 1920, height: 1080 }],
    duration: 10 * F,
    fps: { num: 30, den: 1 },
  },
});

const folder = (id: FolderId, name: string, parent: FolderId | null) => ({
  id,
  name,
  parent,
  custom: {},
});

const collection = (id: CollectionId, name: string, kind: object) => ({
  id,
  name,
  kind,
  custom: {},
});

/**
 * Top level: clip1 to clip3, the folders Interviews (> Day 1, holding inside.mp4) and B-roll
 * (broll-a, broll-b). clip1, inside and broll-a are tagged "keep"; clip2 is on the timeline.
 * Collections: Picks (manual: broll-b, clip1), Keepers (smart: tag keep), Archive (manual, empty).
 */
const project = (extra: Record<string, object> = {}): Project =>
  ({
    meta: { name: "P" },
    settings: {},
    media: {
      med_1: asset("med_1", "clip1.mp4", null, ["keep"]),
      med_2: asset("med_2", "clip2.mp4", null),
      med_3: asset("med_3", "clip3.mp4", null),
      med_4: asset("med_4", "inside.mp4", DAY1, ["keep"]),
      med_5: asset("med_5", "broll-a.mp4", BROLL, ["keep"]),
      med_6: asset("med_6", "broll-b.mp4", BROLL),
    },
    folders: {
      [INTERVIEWS]: folder(INTERVIEWS, "Interviews", null),
      [DAY1]: folder(DAY1, "Day 1", INTERVIEWS),
      [BROLL]: folder(BROLL, "B-roll", null),
    },
    collections: {
      [PICKS]: collection(PICKS, "Picks", { type: "manual", media: ["med_6", "med_1"] }),
      [KEEPERS]: collection(KEEPERS, "Keepers", { type: "smart", filter: { tags: ["keep"] } }),
      [ARCHIVE]: collection(ARCHIVE, "Archive", { type: "manual" }),
      ...extra,
    },
    sequences: {
      seq_1: {
        id: "seq_1",
        name: "Main",
        format: { width: 1920, height: 1080, frame_rate: { num: 30, den: 1 } },
        track_order: ["trk_v"],
        markers: {},
        in_out: null,
        tracks: {
          trk_v: {
            id: "trk_v",
            kind: "video",
            name: "V1",
            items: { itm_0: { kind: "clip", id: "itm_0", media: "med_2" } },
          },
        },
        custom: {},
      },
    },
    custom: {},
  }) as unknown as Project;

const load = (p: Project) =>
  docStore.setState({ status: "ready", rev: 1, committedRev: 1, project: p });

let call: ReturnType<typeof vi.fn>;
function connect(): void {
  call = vi.fn(async () => ({}));
  engineStore.setState({ rpc: { call } as never, connection: "ready" });
}

const view = () =>
  render(h(I18nextProvider, { i18n }, h(Fragment, null, h(MediaPanel), h(MenuDialogs))));

const cardNames = () =>
  within(screen.getByRole("listbox"))
    .getAllByRole("option")
    .map((o) => o.querySelector(".cw-media__name, .cw-foldertile__name")?.textContent);
const card = (name: string) => {
  const found = within(screen.getByRole("listbox"))
    .getAllByRole("option")
    .find((o) => o.querySelector(".cw-media__name")?.textContent === name);
  if (found === undefined) throw new Error(`no card ${name}`);
  return found;
};

const toggle = () => screen.getByTestId("bin-sidebar-toggle");
const sidebar = () => screen.getByRole("complementary", { name: "Folders and collections" });
const folderTree = () => within(sidebar()).getByRole("tree", { name: "Folders" });
const collectionTree = () => within(sidebar()).getByRole("tree", { name: "Collections" });
const rowNames = (tree: HTMLElement) =>
  within(tree)
    .getAllByRole("treeitem")
    .map((row) => row.querySelector(".cw-binside__name")?.textContent);
const folderRow = (name: string) => within(folderTree()).getByRole("treeitem", { name });
const collectionRow = (name: string) => {
  const found = within(collectionTree())
    .getAllByRole("treeitem")
    .find((row) => row.querySelector(".cw-binside__name")?.textContent === name);
  if (found === undefined) throw new Error(`no collection ${name}`);
  return found;
};
const selected = (tree: HTMLElement) =>
  within(tree)
    .getAllByRole("treeitem")
    .filter((row) => row.getAttribute("aria-selected") === "true")
    .map((row) => row.querySelector(".cw-binside__name")?.textContent);

/** What the browser hands to the drag events of a tile drag. */
const transfer = () => ({
  types: [TILE_DRAG_TYPE],
  setData: vi.fn(),
  effectAllowed: "",
  dropEffect: "",
  files: [] as File[],
});

/** The panel's width as the browser would report it (jsdom lays nothing out). */
let panelWidth = 0;
const widthSpy = () =>
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(function (
    this: HTMLElement,
  ) {
    return this.classList.contains("cw-media") ? panelWidth : 0;
  });

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
});

beforeEach(() => {
  panelWidth = 0;
  viewStore.setState({ ...initialViewState, layouts: {} });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  engineStore.setState({ rpc: null, connection: "none" });
  jobStore.getState().reset([]);
  mediaStore.getState().reset();
  binStore.setState({ ...initialBinState, sort: { by: "name", order: "asc" }, sidebar: true });
  folderUiStore.setState({ folders: [], anchor: null, renaming: null, dropOver: undefined });
  sidebarUiStore.setState({ collapsed: new Set(), renaming: null, dropOver: null, overlay: false });
  sessionStore.setState({ path: PATH });
  menuDialogStore.getState().close();
  toastStore.getState().clear();
  dragStore.getState().end();
  tileDragStore.getState().end();
  setThumbnailSource(placeholderThumbnails);
  delete (globalThis as { cutweave?: unknown }).cutweave;
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the sidebar toggle", () => {
  it("shows and hides the sidebar and keeps the choice as editor state", async () => {
    binStore.getState().setSidebar(false);
    load(project());
    view();
    expect(screen.queryByRole("complementary")).toBeNull();
    expect(toggle().getAttribute("aria-pressed")).toBe("false");
    expect(toggle().getAttribute("aria-label")).toBe("Show folders and collections");
    await userEvent.click(toggle());
    expect(sidebar()).toBeDefined();
    expect(sidebar().getAttribute("data-mode")).toBe("inline");
    expect(toggle().getAttribute("aria-pressed")).toBe("true");
    expect(toggle().getAttribute("aria-label")).toBe("Hide folders and collections");
    expect(binStore.getState().sidebar).toBe(true);
    await userEvent.click(toggle());
    expect(screen.queryByRole("complementary")).toBeNull();
    expect(binStore.getState().sidebar).toBe(false);
  });

  it("opens over the grid in a narrow panel and closes once something is opened", async () => {
    panelWidth = SIDEBAR_MIN_PANEL_WIDTH - 40;
    widthSpy();
    load(project());
    view();
    // The kept choice is for a wide panel: a narrow one starts with the grid alone.
    expect(screen.queryByRole("complementary")).toBeNull();
    await userEvent.click(toggle());
    expect(sidebar().getAttribute("data-mode")).toBe("overlay");
    // The keyboard is in it.
    expect(document.activeElement).toBe(folderRow("All media"));
    await userEvent.click(folderRow("B-roll"));
    expect(screen.queryByRole("complementary")).toBeNull();
    expect(cardNames()).toEqual(["broll-a.mp4", "broll-b.mp4"]);
    expect(binStore.getState().sidebar).toBe(true);
    // Escape closes it too, and the focus is back on the toggle.
    await userEvent.click(toggle());
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("complementary")).toBeNull();
    expect(document.activeElement).toBe(toggle());
  });

  it("is beside the grid from the minimum width up", () => {
    panelWidth = SIDEBAR_MIN_PANEL_WIDTH;
    widthSpy();
    load(project());
    view();
    expect(sidebar().getAttribute("data-mode")).toBe("inline");
  });
});

describe("the folder tree", () => {
  it("lists All media and the folders with their subfolders, and marks the open one", async () => {
    load(project());
    view();
    expect(rowNames(folderTree())).toEqual(["All media", "B-roll", "Interviews", "Day 1"]);
    expect(folderRow("Day 1").getAttribute("aria-level")).toBe("2");
    expect(folderRow("Interviews").getAttribute("aria-expanded")).toBe("true");
    expect(folderRow("B-roll").getAttribute("aria-expanded")).toBeNull();
    expect(selected(folderTree())).toEqual(["All media"]);
    await userEvent.click(folderRow("Day 1"));
    expect(selected(folderTree())).toEqual(["Day 1"]);
    expect(cardNames()).toEqual(["inside.mp4"]);
    expect(binStore.getState().openFolders[PATH]).toBe(DAY1);
    await userEvent.click(folderRow("All media"));
    expect(cardNames()).toEqual(["B-roll", "Interviews", "clip1.mp4", "clip2.mp4", "clip3.mp4"]);
  });

  it("moves with the arrow keys, opens with Enter and closes a folder with Left", async () => {
    load(project());
    view();
    folderRow("All media").focus();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}");
    expect(document.activeElement).toBe(folderRow("Interviews"));
    // One tab stop: the row the keyboard is on.
    expect(
      within(folderTree())
        .getAllByRole("treeitem")
        .filter((row) => row.tabIndex === 0),
    ).toEqual([folderRow("Interviews")]);
    await userEvent.keyboard("{ArrowLeft}");
    expect(folderRow("Interviews").getAttribute("aria-expanded")).toBe("false");
    expect(rowNames(folderTree())).toEqual(["All media", "B-roll", "Interviews"]);
    await userEvent.keyboard("{ArrowRight}");
    expect(rowNames(folderTree())).toEqual(["All media", "B-roll", "Interviews", "Day 1"]);
    await userEvent.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(folderRow("Day 1"));
    await userEvent.keyboard("{Enter}");
    expect(binStore.getState().openFolders[PATH]).toBe(DAY1);
    await userEvent.keyboard("{Home}{Enter}");
    expect(binStore.getState().openFolders[PATH]).toBeUndefined();
  });

  it("takes dropped cards on a folder row as one media.move, like the breadcrumb", async () => {
    const user = userEvent.setup();
    load(project());
    connect();
    call.mockResolvedValue({ to: BROLL, media: ["med_1", "med_3"], folders: [], renamed: [] });
    view();
    await user.click(card("clip1.mp4"));
    await user.keyboard("{Control>}");
    await user.click(card("clip3.mp4"));
    await user.keyboard("{/Control}");
    const data = transfer();
    fireEvent.dragStart(card("clip1.mp4"), { dataTransfer: data });
    const target = folderRow("B-roll");
    expect(target.getAttribute("data-drop-folder")).toBe(BROLL);
    fireEvent.dragOver(target, { dataTransfer: data });
    expect(target.getAttribute("data-drop-target")).toBe("true");
    expect(data.dropEffect).toBe("move");
    fireEvent.drop(target, { dataTransfer: data });
    expect(call).toHaveBeenCalledWith("media.move", {
      media: ["med_1", "med_3"],
      to: BROLL,
      on_conflict: "rename",
    });
    expect(tileDragStore.getState().payload).toBeNull();
    expect(target.getAttribute("data-drop-target")).toBeNull();
  });

  it("refuses a drop that changes nothing, and takes one on All media from inside a folder", async () => {
    load(project());
    connect();
    view();
    const data = transfer();
    fireEvent.dragStart(card("clip1.mp4"), { dataTransfer: data });
    // It is at the top level already.
    expect(fireEvent.dragOver(folderRow("All media"), { dataTransfer: data })).toBe(true);
    fireEvent.drop(folderRow("All media"), { dataTransfer: data });
    expect(call).not.toHaveBeenCalled();
    fireEvent.dragEnd(card("clip1.mp4"));

    await userEvent.click(folderRow("B-roll"));
    const out = transfer();
    fireEvent.dragStart(card("broll-a.mp4"), { dataTransfer: out });
    expect(fireEvent.dragOver(folderRow("All media"), { dataTransfer: out })).toBe(false);
    fireEvent.drop(folderRow("All media"), { dataTransfer: out });
    expect(call).toHaveBeenCalledWith("media.move", {
      media: ["med_5"],
      to: null,
      on_conflict: "rename",
    });
  });
});

describe("the collections", () => {
  it("lists them by name with their kind and count", () => {
    load(project());
    view();
    expect(rowNames(collectionTree())).toEqual(["Archive", "Keepers", "Picks"]);
    expect(collectionRow("Archive").getAttribute("aria-label")).toBe("Archive, collection, empty");
    expect(collectionRow("Keepers").getAttribute("aria-label")).toBe(
      "Keepers, smart collection, 3 items",
    );
    expect(collectionRow("Picks").getAttribute("data-collection-kind")).toBe("manual");
    expect(collectionRow("Picks").querySelector(".cw-binside__count")?.textContent).toBe("2");
  });

  it("says so when there are none", () => {
    const p = project();
    delete (p as { collections?: unknown }).collections;
    load(p);
    view();
    expect(within(sidebar()).queryByRole("tree", { name: "Collections" })).toBeNull();
    expect(within(sidebar()).getByText(/No collections yet/)).toBeDefined();
  });

  it("opens a manual collection: its members in their stored order, with where each one is", async () => {
    load(project());
    view();
    await userEvent.click(collectionRow("Picks"));
    expect(cardNames()).toEqual(["broll-b.mp4", "clip1.mp4"]);
    expect(card("broll-b.mp4").querySelector(".cw-media__path")?.textContent).toBe("B-roll");
    expect(selected(collectionTree())).toEqual(["Picks"]);
    // The one marked row: no folder is the open place now.
    expect(selected(folderTree())).toEqual([]);
    expect(binStore.getState().openCollections[PATH]).toBe(PICKS);
    const nav = screen.getByRole("navigation", { name: "Media folders" });
    expect(
      within(nav)
        .getAllByRole("button")
        .map((b) => b.textContent),
    ).toEqual(["Media", "Picks"]);
    // The order is the collection's own: sorting is off, and says why.
    const sort = screen.getByRole("button", {
      name: "A collection keeps the order its clips were added in",
    });
    expect((sort as HTMLButtonElement).disabled).toBe(true);
    // The breadcrumb leads back to the folders.
    await userEvent.click(within(nav).getByRole("button", { name: "Media" }));
    expect(binStore.getState().openCollections[PATH]).toBeUndefined();
    expect(selected(folderTree())).toEqual(["All media"]);
  });

  it("opens a smart collection: what passes its filter now, sorted as the bin is", async () => {
    load(project());
    view();
    await userEvent.click(collectionRow("Keepers"));
    expect(cardNames()).toEqual(["broll-a.mp4", "clip1.mp4", "inside.mp4"]);
    expect(card("inside.mp4").querySelector(".cw-media__path")?.textContent).toBe(
      "Interviews / Day 1",
    );
    // A clip that gets the tag joins it.
    const p = project();
    (p.media as Record<string, { tags: string[] }>).med_3.tags = ["keep"];
    load(p);
    await vi.waitFor(() =>
      expect(cardNames()).toEqual(["broll-a.mp4", "clip1.mp4", "clip3.mp4", "inside.mp4"]),
    );
    expect(screen.getByRole("button", { name: "Sort" })).toBeDefined();
  });

  it("shows an empty collection as empty, with what to do", async () => {
    load(project());
    view();
    await userEvent.click(collectionRow("Archive"));
    expect(screen.getByText("This collection is empty")).toBeDefined();
    expect(screen.getByText(/Drag clips onto it in the sidebar/)).toBeDefined();
  });

  it("falls back to the folders when the open collection is deleted", async () => {
    load(project());
    view();
    await userEvent.click(collectionRow("Picks"));
    const p = project();
    delete (p.collections as Record<string, unknown>)[PICKS];
    load(p);
    await vi.waitFor(() =>
      expect(cardNames()).toEqual(["B-roll", "Interviews", "clip1.mp4", "clip2.mp4", "clip3.mp4"]),
    );
    expect(selected(folderTree())).toEqual(["All media"]);
  });

  it("New collection creates one under a free name and renames it in place", async () => {
    load(project());
    connect();
    call.mockResolvedValue({ collection_id: "col_new", name: "New collection" });
    view();
    await userEvent.click(within(sidebar()).getByRole("button", { name: "New collection" }));
    expect(call).toHaveBeenCalledWith("media.create_collection", {
      name: "New collection",
      kind: { type: "manual", media: [] },
    });
    // The document arrives with it: its name is being edited.
    load(
      project({
        col_new: collection("col_new" as CollectionId, "New collection", { type: "manual" }),
      }),
    );
    const field = await screen.findByRole("textbox", { name: "Collection name" });
    expect((field as HTMLInputElement).value).toBe("New collection");
    call.mockClear();
    await userEvent.clear(field);
    await userEvent.type(field, "Selects{Enter}");
    expect(call).toHaveBeenCalledWith("media.rename_collection", {
      collection: "col_new",
      name: "Selects",
    });
    expect(screen.queryByRole("textbox", { name: "Collection name" })).toBeNull();
  });

  it("renames a collection with F2 and leaves it alone on Escape", async () => {
    load(project());
    connect();
    view();
    collectionRow("Archive").focus();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}");
    expect(document.activeElement).toBe(collectionRow("Picks"));
    await userEvent.keyboard("{F2}");
    const field = await screen.findByRole("textbox", { name: "Collection name" });
    await userEvent.type(field, "x{Escape}");
    expect(call).not.toHaveBeenCalled();
    expect(screen.queryByRole("textbox", { name: "Collection name" })).toBeNull();
    // The keyboard is back on the row.
    await vi.waitFor(() => expect(document.activeElement).toBe(collectionRow("Picks")));
    await userEvent.keyboard("{Enter}");
    expect(binStore.getState().openCollections[PATH]).toBe(PICKS);
  });

  it("Delete asks before deleting a collection with members, and says the media stay", async () => {
    load(project());
    connect();
    view();
    collectionRow("Picks").focus();
    await userEvent.keyboard("{Delete}");
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Delete the collection Picks?")).toBeDefined();
    expect(within(dialog).getByText("It lists 2 items.")).toBeDefined();
    expect(
      within(dialog).getByText(/The media stay in the project and in their folders/),
    ).toBeDefined();
    expect(call).not.toHaveBeenCalled();
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete collection" }));
    expect(call).toHaveBeenCalledWith("media.delete_collection", { collection: PICKS });
  });

  it("Delete removes an empty collection without asking", async () => {
    load(project());
    connect();
    view();
    collectionRow("Archive").focus();
    await userEvent.keyboard("{Delete}");
    expect(call).toHaveBeenCalledWith("media.delete_collection", { collection: ARCHIVE });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("adds cards dropped on a manual collection: only the ones it does not hold", async () => {
    const user = userEvent.setup();
    load(project());
    connect();
    call.mockResolvedValue({ collection_id: PICKS, added: ["med_3"], members: 3 });
    view();
    await user.click(card("clip1.mp4"));
    await user.keyboard("{Control>}");
    await user.click(card("clip3.mp4"));
    await user.keyboard("{/Control}");
    const data = transfer();
    fireEvent.dragStart(card("clip1.mp4"), { dataTransfer: data });
    const target = collectionRow("Picks");
    // Accepted (the default is prevented), as a copy: the cards stay where they are.
    expect(fireEvent.dragOver(target, { dataTransfer: data })).toBe(false);
    expect(data.dropEffect).toBe("copy");
    expect(target.getAttribute("data-drop-target")).toBe("true");
    fireEvent.drop(target, { dataTransfer: data });
    // clip1 is on it already.
    expect(call).toHaveBeenCalledWith("media.add_to_collection", {
      collection: PICKS,
      media: ["med_3"],
    });
    expect(call).toHaveBeenCalledTimes(1);
    expect(target.getAttribute("data-drop-target")).toBeNull();
    expect(tileDragStore.getState().payload).toBeNull();
    await vi.waitFor(() =>
      expect(toastStore.getState().notices.map((n) => n.title)).toEqual(["Added 1 item to Picks"]),
    );
  });

  it("is not a place to drop for a smart collection, or for cards a collection holds", () => {
    load(project());
    connect();
    view();
    const data = transfer();
    fireEvent.dragStart(card("clip1.mp4"), { dataTransfer: data });
    expect(fireEvent.dragOver(collectionRow("Keepers"), { dataTransfer: data })).toBe(true);
    expect(fireEvent.dragOver(collectionRow("Picks"), { dataTransfer: data })).toBe(true);
    fireEvent.drop(collectionRow("Picks"), { dataTransfer: data });
    expect(call).not.toHaveBeenCalled();
  });
});

describe("Save as smart collection", () => {
  it("is offered while a search or a filter is on, and stores them under a name", async () => {
    load(project());
    connect();
    call.mockResolvedValue({ collection_id: "col_s", name: "keep" });
    view();
    expect(screen.queryByRole("button", { name: "Save as smart collection…" })).toBeNull();
    binStore.getState().toggleTag("keep");
    binStore.getState().toggleKind("video");
    await userEvent.click(await screen.findByRole("button", { name: "Save as smart collection…" }));
    const dialog = await screen.findByRole("dialog");
    const field = within(dialog).getByRole("textbox", { name: "Collection name" });
    // It opens with the tag as the name.
    expect((field as HTMLInputElement).value).toBe("keep");
    await userEvent.clear(field);
    await userEvent.type(field, "Keep video");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(call).toHaveBeenCalledWith("media.create_collection", {
      name: "Keep video",
      kind: { type: "smart", filter: { kinds: ["video"], tags: ["keep"] } },
    });
    await vi.waitFor(() =>
      expect(toastStore.getState().notices.map((n) => n.title)).toEqual([
        "Saved Keep video. It is in the sidebar of the media bin.",
      ]),
    );
  });

  it("includes the open folder when the search is scoped to This folder", async () => {
    load(project());
    connect();
    view();
    await userEvent.click(folderRow("Interviews"));
    binStore.getState().setScope("folder");
    binStore.getState().setText("inside");
    await userEvent.click(await screen.findByRole("button", { name: "Save as smart collection…" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(call).toHaveBeenCalledWith("media.create_collection", {
      name: "inside",
      kind: { type: "smart", filter: { text: "inside", folder: INTERVIEWS, recursive: true } },
    });
  });

  it("does not save under a name another collection has", async () => {
    load(project());
    connect();
    view();
    binStore.getState().setText("clip");
    await userEvent.click(await screen.findByRole("button", { name: "Save as smart collection…" }));
    const dialog = await screen.findByRole("dialog");
    const field = within(dialog).getByRole("textbox", { name: "Collection name" });
    await userEvent.clear(field);
    await userEvent.type(field, "picks");
    expect(within(dialog).getByText("A collection has this name already.")).toBeDefined();
    expect(
      (within(dialog).getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    await userEvent.keyboard("{Enter}");
    expect(call).not.toHaveBeenCalled();
  });

  it("is not offered inside a collection: a smart collection cannot name one", async () => {
    load(project());
    connect();
    view();
    await userEvent.click(collectionRow("Keepers"));
    binStore.getState().setText("clip");
    await vi.waitFor(() => expect(cardNames()).toEqual(["clip1.mp4"]));
    expect(screen.queryByRole("button", { name: "Save as smart collection…" })).toBeNull();
  });
});

describe("the Used badge", () => {
  it("marks the cards a timeline uses, and only those", () => {
    load(project());
    view();
    const used = within(card("clip2.mp4")).getByTestId("used-badge");
    expect(used.textContent).toBe("Used");
    expect(used.getAttribute("title")).toBe("On a timeline: 1 clip uses it");
    expect(within(card("clip1.mp4")).queryByTestId("used-badge")).toBeNull();
  });
});

describe("sidebar accessibility", () => {
  it("has no axe violations with the trees, an open collection, a rename and the dialogs", async () => {
    load(project());
    connect();
    const { container } = view();
    expect((await axe.run(container)).violations).toEqual([]);
    await userEvent.click(collectionRow("Picks"));
    expect((await axe.run(container)).violations).toEqual([]);
    collectionRow("Picks").focus();
    await userEvent.keyboard("{F2}");
    await screen.findByRole("textbox", { name: "Collection name" });
    expect((await axe.run(container)).violations).toEqual([]);
    await userEvent.keyboard("{Escape}");
    collectionRow("Picks").focus();
    await userEvent.keyboard("{Delete}");
    expect((await axe.run(await screen.findByRole("dialog"))).violations).toEqual([]);
    await userEvent.keyboard("{Escape}");
    await userEvent.click(folderRow("All media"));
    binStore.getState().setText("clip");
    await userEvent.click(await screen.findByRole("button", { name: "Save as smart collection…" }));
    expect((await axe.run(await screen.findByRole("dialog"))).violations).toEqual([]);
  });
});

describe("undo labels", () => {
  const t = translator(i18n.t.bind(i18n));

  it("names the six collection commands", () => {
    expect(historyLabel("media.create_collection", t)).toBe("New collection");
    expect(historyLabel("media.rename_collection", t)).toBe("Rename collection");
    expect(historyLabel("media.delete_collection", t)).toBe("Delete collection");
    expect(historyLabel("media.add_to_collection", t)).toBe("Add to collection");
    expect(historyLabel("media.remove_from_collection", t)).toBe("Remove from collection");
    expect(historyLabel("media.set_collection_filter", t)).toBe("Update smart collection");
    expect(
      undoText(
        {
          canUndo: true,
          canRedo: false,
          nextUndo: "media.add_to_collection",
          nextRedo: null,
          nextUndoActor: "user",
          nextRedoActor: null,
        } as never,
        t,
      ),
    ).toBe("Undo Add to collection");
  });
});

describe("the palette's New collection… and Add to collection…", () => {
  it("ask for a name, and list the manual collections for the selected cards", async () => {
    registerBuiltinPaletteActions();
    const create = paletteAction("media.create_collection");
    const add = paletteAction("media.add_to_collection");
    expect(create).toBeDefined();
    expect(add).toBeDefined();
    load(project());
    connect();
    call.mockResolvedValue({ collection_id: PICKS, added: ["med_2", "med_3"], members: 4 });
    view();
    // Nothing selected: there is nothing to add, and the palette says so.
    expect(() => add?.({})).toThrow("Select media in the bin first.");
    await create?.({});
    expect(menuDialogStore.getState().dialog).toEqual({
      kind: "new-collection",
      media: [],
      filter: null,
    });
    menuDialogStore.getState().close();

    viewStore.getState().setSelection([
      { kind: "media", id: "med_2" },
      { kind: "media", id: "med_3" },
    ] as never);
    await add?.({});
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Add 2 items to a collection")).toBeDefined();
    // The manual ones; a smart collection takes no members.
    expect(
      within(dialog)
        .getAllByRole("radio")
        .map((r) => r.closest("label")?.textContent),
    ).toEqual(["Archive", "Picks"]);
    expect((await axe.run(dialog)).violations).toEqual([]);
    await userEvent.click(within(dialog).getByRole("radio", { name: "Picks" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(call).toHaveBeenCalledWith("media.add_to_collection", {
      collection: PICKS,
      media: ["med_2", "med_3"],
    });
  });

  it("New collection… from the list makes a collection of the selected cards", async () => {
    registerBuiltinPaletteActions();
    load(project());
    connect();
    call.mockResolvedValue({ collection_id: "col_new", name: "Day 2" });
    view();
    viewStore.getState().setSelection([{ kind: "media", id: "med_2" }] as never);
    await paletteAction("media.add_to_collection")?.({});
    await userEvent.click(
      within(await screen.findByRole("dialog")).getByRole("button", { name: "New collection…" }),
    );
    const field = await screen.findByRole("textbox", { name: "Collection name" });
    expect(
      screen.getByText("The selected item goes into it and stays in its folder."),
    ).toBeDefined();
    await userEvent.clear(field);
    await userEvent.type(field, "Day 2{Enter}");
    expect(call).toHaveBeenCalledWith("media.create_collection", {
      name: "Day 2",
      kind: { type: "manual", media: ["med_2"] },
    });
  });
});
