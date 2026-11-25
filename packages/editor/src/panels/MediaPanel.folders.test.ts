// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The media bin's folders (WP-2F.08): folder tiles before assets, the breadcrumb, opening and
// leaving a folder, multi-select, dropping tiles on a folder tile and on the breadcrumb, importing
// into the open folder, the search scope, and deleting a folder (the dialog's two answers).
import type { FolderId, MediaId, Project } from "@cutweave/protocol";
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
import { placeholderThumbnails, setThumbnailSource } from "../media/thumbnails.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { MenuDialogs } from "../menus/MenuDialogs.tsx";
import { sessionStore } from "../session/sessionStore.ts";
import { docStore } from "../state/docStore.ts";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { MediaPanel } from "./MediaPanel.tsx";

const F = 705_600_000;
const INTERVIEWS = "fld_interviews" as FolderId;
const DAY1 = "fld_day1" as FolderId;
const BROLL = "fld_broll" as FolderId;
const EMPTY = "fld_empty" as FolderId;

const asset = (
  id: string,
  name: string,
  folder: FolderId | null,
  display: string | null = null,
) => ({
  id,
  kind: "video",
  path_abs: `/media/${name}`,
  path_rel: null,
  name: display,
  folder,
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

/** Top level: clip1 to clip3 and the folders Interviews (> Day 1), B-roll and Empty. */
const project = (used: string[] = []): Project =>
  ({
    meta: { name: "P" },
    settings: {},
    media: {
      med_1: asset("med_1", "clip1.mp4", null),
      med_2: asset("med_2", "clip2.mp4", null),
      med_3: asset("med_3", "clip3.mp4", null),
      med_4: asset("med_4", "inside.mp4", INTERVIEWS),
      med_5: asset("med_5", "broll-a.mp4", BROLL),
      med_6: asset("med_6", "broll-b.mp4", BROLL),
    },
    folders: {
      [INTERVIEWS]: folder(INTERVIEWS, "Interviews", null),
      [DAY1]: folder(DAY1, "Day 1", INTERVIEWS),
      [BROLL]: folder(BROLL, "B-roll", null),
      [EMPTY]: folder(EMPTY, "Empty", null),
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
            items: Object.fromEntries(
              used.map((media, i) => [`itm_${i}`, { kind: "clip", id: `itm_${i}`, media }]),
            ),
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

const options = () => within(screen.getByRole("listbox")).getAllByRole("option");
const optionNames = () =>
  options().map((o) => o.querySelector(".cw-media__name, .cw-foldertile__name")?.textContent);
const tile = (name: string) => {
  const found = options().find(
    (o) => o.querySelector(".cw-media__name, .cw-foldertile__name")?.textContent === name,
  );
  if (found === undefined) throw new Error(`no tile ${name}: ${optionNames().join(", ")}`);
  return found;
};

/** What the browser hands to the drag events of a tile drag. */
const transfer = () => ({
  types: [TILE_DRAG_TYPE],
  setData: vi.fn(),
  effectAllowed: "",
  dropEffect: "",
  files: [] as File[],
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
  viewStore.setState({ ...initialViewState, layouts: {} });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  engineStore.setState({ rpc: null, connection: "none" });
  jobStore.getState().reset([]);
  mediaStore.getState().reset();
  binStore.setState({ ...initialBinState, sort: { by: "imported", order: "asc" } });
  folderUiStore.setState({ folders: [], anchor: null, renaming: null, dropOver: undefined });
  sessionStore.setState({ path: "C:/p/film.cutweave" });
  menuDialogStore.getState().close();
  toastStore.getState().clear();
  dragStore.getState().end();
  tileDragStore.getState().end();
  setThumbnailSource(placeholderThumbnails);
  delete (globalThis as { cutweave?: unknown }).cutweave;
});
afterEach(cleanup);

describe("browsing folders", () => {
  it("shows the subfolders first, with their item counts, then the assets", () => {
    load(project());
    view();
    expect(optionNames()).toEqual([
      "B-roll",
      "Empty",
      "Interviews",
      "clip1.mp4",
      "clip2.mp4",
      "clip3.mp4",
    ]);
    expect(tile("B-roll").getAttribute("aria-label")).toBe("B-roll, folder, 2 items");
    expect(tile("Empty").getAttribute("aria-label")).toBe("Empty, folder, empty");
    // Interviews holds one asset and the folder Day 1.
    expect(tile("Interviews").getAttribute("aria-label")).toBe("Interviews, folder, 2 items");
  });

  it("opens a folder with a double-click and shows Media > Interviews in the breadcrumb", async () => {
    load(project());
    view();
    await userEvent.dblClick(tile("Interviews"));
    expect(optionNames()).toEqual(["Day 1", "inside.mp4"]);
    const nav = screen.getByRole("navigation", { name: "Media folders" });
    expect(
      within(nav)
        .getAllByRole("button")
        .map((b) => b.textContent),
    ).toEqual(["Media", "Interviews"]);
    expect(
      within(nav).getByRole("button", { name: "Interviews" }).getAttribute("aria-current"),
    ).toBe("page");
    expect(binStore.getState().openFolders["C:/p/film.cutweave"]).toBe(INTERVIEWS);
  });

  it("opens with Enter and goes up with Backspace, Alt+Up and the breadcrumb", async () => {
    load(project());
    view();
    tile("Interviews").focus();
    await userEvent.keyboard("{Enter}");
    expect(optionNames()).toEqual(["Day 1", "inside.mp4"]);
    options()[0]?.focus();
    await userEvent.keyboard("{Backspace}");
    expect(optionNames()[0]).toBe("B-roll");
    tile("Interviews").focus();
    await userEvent.keyboard("{Enter}");
    options()[1]?.focus();
    await userEvent.keyboard("{Alt>}{ArrowUp}{/Alt}");
    expect(optionNames()[0]).toBe("B-roll");
    await userEvent.dblClick(tile("Interviews"));
    await userEvent.click(screen.getByRole("button", { name: "Media" }));
    expect(optionNames()[0]).toBe("B-roll");
  });

  it("keeps the open folder per project and falls back to the top level when it is gone", () => {
    load(project());
    binStore.getState().openFolder("C:/p/film.cutweave", BROLL);
    view();
    expect(optionNames()).toEqual(["broll-a.mp4", "broll-b.mp4"]);
    cleanup();
    sessionStore.setState({ path: "C:/p/other.cutweave" });
    view();
    expect(optionNames()[0]).toBe("B-roll");
    cleanup();
    binStore.getState().openFolder("C:/p/other.cutweave", "fld_gone" as FolderId);
    view();
    expect(optionNames()[0]).toBe("B-roll");
  });

  it("says an open folder with nothing in it is empty", () => {
    load(project());
    binStore.getState().openFolder("C:/p/film.cutweave", EMPTY);
    view();
    expect(screen.getByText("This folder is empty")).toBeTruthy();
  });
});

describe("selecting several tiles", () => {
  it("Ctrl-click adds, Shift-click takes a range, Ctrl+A takes everything", async () => {
    const user = userEvent.setup();
    load(project());
    view();
    await user.click(tile("clip1.mp4"));
    await user.keyboard("{Control>}");
    await user.click(tile("clip3.mp4"));
    await user.keyboard("{/Control}");
    expect(viewStore.getState().selection.map((r) => r.id)).toEqual(["med_1", "med_3"]);
    await user.click(tile("B-roll"));
    await user.keyboard("{Shift>}");
    await user.click(tile("clip2.mp4"));
    await user.keyboard("{/Shift}");
    expect(folderUiStore.getState().folders).toEqual([BROLL, EMPTY, INTERVIEWS]);
    expect(viewStore.getState().selection.map((r) => r.id)).toEqual(["med_1", "med_2"]);
    tile("clip1.mp4").focus();
    await user.keyboard("{Control>}a{/Control}");
    expect(options().filter((o) => o.getAttribute("aria-selected") === "true")).toHaveLength(6);
  });
});

describe("dragging tiles", () => {
  it("drops the selected assets on a folder tile as one media.move, renaming on a clash", async () => {
    const user = userEvent.setup();
    load(project());
    connect();
    call.mockResolvedValue({ to: BROLL, media: ["med_1", "med_3"], folders: [], renamed: [] });
    view();
    await user.click(tile("clip1.mp4"));
    await user.keyboard("{Control>}");
    await user.click(tile("clip3.mp4"));
    await user.keyboard("{/Control}");
    const data = transfer();
    fireEvent.dragStart(tile("clip1.mp4"), { dataTransfer: data });
    expect(tileDragStore.getState().payload).toEqual({ media: ["med_1", "med_3"], folders: [] });
    // The timeline still sees the asset that was dragged.
    expect(dragStore.getState().session?.id).toBe("med_1");
    const target = tile("B-roll");
    fireEvent.dragOver(target, { dataTransfer: data });
    expect(target.getAttribute("data-drop-target")).toBe("true");
    fireEvent.drop(target, { dataTransfer: data });
    expect(call).toHaveBeenCalledWith("media.move", {
      media: ["med_1", "med_3"],
      to: BROLL,
      on_conflict: "rename",
    });
    expect(tileDragStore.getState().payload).toBeNull();
    expect(target.getAttribute("data-drop-target")).toBeNull();
  });

  it("tells when folders were renamed to fit", async () => {
    const user = userEvent.setup();
    load(project());
    connect();
    call.mockResolvedValue({
      to: BROLL,
      media: [],
      folders: [EMPTY],
      renamed: [{ folder: EMPTY, from: "Empty", to: "Empty 2" }],
    });
    view();
    await user.click(tile("Empty"));
    const data = transfer();
    fireEvent.dragStart(tile("Empty"), { dataTransfer: data });
    fireEvent.drop(tile("B-roll"), { dataTransfer: data });
    await vi.waitFor(() =>
      expect(toastStore.getState().notices.map((n) => n.title)).toEqual([
        "Moved. 1 folder was renamed because its name was taken.",
      ]),
    );
  });

  it("does not take a drop that would change nothing or put a folder inside itself", async () => {
    const user = userEvent.setup();
    load(project());
    connect();
    view();
    await user.dblClick(tile("Interviews"));
    await user.click(tile("inside.mp4"));
    const data = transfer();
    fireEvent.dragStart(tile("inside.mp4"), { dataTransfer: data });
    // It is in Interviews already: dropping on that breadcrumb segment is not a place to drop.
    const crumb = screen.getByRole("button", { name: "Interviews" });
    const accepted = fireEvent.dragOver(crumb, { dataTransfer: data });
    expect(accepted).toBe(true);
    fireEvent.drop(crumb, { dataTransfer: data });
    expect(call).not.toHaveBeenCalled();
    fireEvent.dragEnd(tile("inside.mp4"));
    // A folder dropped on itself.
    await user.click(tile("Day 1"));
    const folderData = transfer();
    fireEvent.dragStart(tile("Day 1"), { dataTransfer: folderData });
    fireEvent.drop(tile("Day 1"), { dataTransfer: folderData });
    expect(call).not.toHaveBeenCalled();
  });

  it("drops an asset on a breadcrumb segment, the top level included", async () => {
    const user = userEvent.setup();
    load(project());
    connect();
    call.mockResolvedValue({ to: null, media: ["med_4"], folders: [], renamed: [] });
    view();
    await user.dblClick(tile("Interviews"));
    await user.click(tile("inside.mp4"));
    const data = transfer();
    fireEvent.dragStart(tile("inside.mp4"), { dataTransfer: data });
    const root = screen.getByRole("button", { name: "Media" });
    fireEvent.dragOver(root, { dataTransfer: data });
    expect(root.getAttribute("data-drop-target")).toBe("true");
    fireEvent.drop(root, { dataTransfer: data });
    expect(call).toHaveBeenCalledWith("media.move", {
      media: ["med_4"],
      to: null,
      on_conflict: "rename",
    });
  });
});

describe("importing into folders", () => {
  it("imports files dropped on the bin into the open folder and on a folder tile into it", async () => {
    load(project());
    connect();
    (globalThis as { cutweave?: unknown }).cutweave = {
      files: { pathForFile: (f: File) => `/dropped/${f.name}` },
    };
    call.mockResolvedValue({ media: [], media_ids: [], failed: [], jobs: [] });
    binStore.getState().openFolder("C:/p/film.cutweave", BROLL);
    const { container } = view();
    const files = [new File(["x"], "a.mp4")];
    const region = container.querySelector(".cw-media") as HTMLElement;
    fireEvent.drop(region, { dataTransfer: { types: ["Files"], files, dropEffect: "" } });
    await vi.waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/dropped/a.mp4"],
        folder: BROLL,
      }),
    );
    call.mockClear();
    await userEvent.click(screen.getByRole("button", { name: "Media" }));
    fireEvent.drop(tile("Interviews"), {
      dataTransfer: { types: ["Files"], files, dropEffect: "" },
    });
    await vi.waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/dropped/a.mp4"],
        folder: INTERVIEWS,
      }),
    );
  });
});

describe("double-clicking the empty body imports (#162)", () => {
  it("opens the picker into the open folder, but a folder tile and a card keep their own", async () => {
    load(project());
    connect();
    const pick = vi.fn(async () => ["/x/a.mp4"]);
    (globalThis as { cutweave?: unknown }).cutweave = { dialog: { pickMediaFiles: pick } };
    call.mockResolvedValue({ media: [], media_ids: [], failed: [], jobs: [] });
    binStore.getState().openFolder("C:/p/film.cutweave", EMPTY);
    view();
    await userEvent.dblClick(screen.getByText("This folder is empty"));
    await vi.waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", { paths: ["/x/a.mp4"], folder: EMPTY }),
    );
    cleanup();
    call.mockClear();
    pick.mockClear();
    binStore.getState().openFolder("C:/p/film.cutweave", null);
    view();
    await userEvent.dblClick(tile("Interviews"));
    expect(pick).not.toHaveBeenCalled();
    expect(optionNames()).toEqual(["Day 1", "inside.mp4"]);
  });
});

describe("search and folders", () => {
  it("All media is flat, with the folder path under each name and matching folders first", async () => {
    load(project());
    view();
    binStore.getState().setText("inter");
    await vi.waitFor(() => expect(optionNames()).toEqual(["Interviews"]));
    binStore.getState().setText("clip");
    await vi.waitFor(() => expect(optionNames()).toEqual(["clip1.mp4", "clip2.mp4", "clip3.mp4"]));
    binStore.getState().setText("inside");
    await vi.waitFor(() => expect(optionNames()).toEqual(["inside.mp4"]));
    expect(options()[0]?.textContent).toContain("Interviews");
  });

  it("This folder looks only inside the open folder and below", async () => {
    load(project());
    binStore.getState().openFolder("C:/p/film.cutweave", INTERVIEWS);
    view();
    binStore.getState().setText("mp4");
    await vi.waitFor(() => expect(optionNames()).toHaveLength(6));
    await userEvent.click(screen.getByRole("radio", { name: "This folder" }));
    await vi.waitFor(() => expect(optionNames()).toEqual(["inside.mp4"]));
  });
});

describe("deleting a folder", () => {
  it("deletes an empty folder at once, as media.delete_folder", async () => {
    load(project());
    connect();
    view();
    await userEvent.click(tile("Empty"));
    await userEvent.keyboard("{Delete}");
    expect(call).toHaveBeenCalledWith("media.delete_folder", {
      folder: EMPTY,
      contents: "move_up",
    });
    expect(menuDialogStore.getState().dialog).toBeNull();
  });

  it("asks what to do with what a folder holds, and Keep media lifts it", async () => {
    load(project(["med_5"]));
    connect();
    view();
    await userEvent.click(tile("B-roll"));
    await userEvent.keyboard("{Delete}");
    expect(call).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Delete \u201cB-roll\u201d and its 2 media?")).toBeTruthy();
    expect(
      within(dialog).getByText("1 is used on the timeline and will move to \u201cMedia\u201d."),
    ).toBeTruthy();
    await userEvent.click(within(dialog).getByRole("button", { name: "Keep media" }));
    expect(call).toHaveBeenCalledWith("media.delete_folder", {
      folder: BROLL,
      contents: "move_up",
    });
  });

  it("Delete folder and media removes the unused media with it", async () => {
    load(project());
    connect();
    view();
    await userEvent.click(tile("B-roll"));
    await userEvent.click(screen.getByRole("button", { name: "Delete folder" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete folder and media" }));
    expect(call).toHaveBeenCalledWith("media.delete_folder", { folder: BROLL, contents: "remove" });
  });

  it("Cancel does nothing", async () => {
    load(project());
    connect();
    view();
    await userEvent.click(tile("B-roll"));
    await userEvent.keyboard("{Delete}");
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(call).not.toHaveBeenCalled();
  });
});

describe("renaming and creating", () => {
  it("F2 renames a folder in place with media.rename_folder", async () => {
    load(project());
    connect();
    view();
    await userEvent.click(tile("B-roll"));
    tile("B-roll").focus();
    await userEvent.keyboard("{F2}");
    const field = screen.getByRole("textbox", { name: "Folder name" });
    await userEvent.clear(field);
    await userEvent.type(field, "Cutaways{Enter}");
    expect(call).toHaveBeenCalledWith("media.rename_folder", { folder: BROLL, name: "Cutaways" });
  });

  it("F2 renames an asset with media.rename, and Escape keeps the name", async () => {
    load(project());
    connect();
    view();
    tile("clip1.mp4").focus();
    await userEvent.keyboard("{F2}");
    const field = screen.getByRole("textbox", { name: "Media name" });
    await userEvent.type(field, "{Escape}");
    expect(call).not.toHaveBeenCalled();
    tile("clip1.mp4").focus();
    await userEvent.keyboard("{F2}");
    const again = screen.getByRole("textbox", { name: "Media name" });
    await userEvent.clear(again);
    await userEvent.type(again, "Opening{Enter}");
    expect(call).toHaveBeenCalledWith("media.rename", {
      media: "med_1" as MediaId,
      name: "Opening",
    });
  });

  it("Ctrl+Shift+N creates a folder from the keyboard", async () => {
    load(project());
    connect();
    call.mockResolvedValue({ folder_id: "fld_k", name: "New folder", parent: null });
    view();
    tile("clip1.mp4").focus();
    await userEvent.keyboard("{Control>}{Shift>}n{/Shift}{/Control}");
    expect(call).toHaveBeenCalledWith("media.create_folder", {
      parent: null,
      on_conflict: "rename",
    });
  });

  it("New folder creates in the open folder and starts renaming it", async () => {
    load(project());
    connect();
    binStore.getState().openFolder("C:/p/film.cutweave", INTERVIEWS);
    call.mockImplementation(async (name: string) => {
      if (name !== "media.create_folder") return {};
      const next = project();
      (next.folders as Record<string, unknown>).fld_new = folder(
        "fld_new" as FolderId,
        "New folder",
        INTERVIEWS,
      );
      docStore.setState({ status: "ready", rev: 2, committedRev: 2, project: next });
      return { folder_id: "fld_new", name: "New folder", parent: INTERVIEWS };
    });
    view();
    await userEvent.click(screen.getByRole("button", { name: "New folder" }));
    expect(call).toHaveBeenCalledWith("media.create_folder", {
      parent: INTERVIEWS,
      on_conflict: "rename",
    });
    const field = await screen.findByRole("textbox", { name: "Folder name" });
    expect((field as HTMLInputElement).value).toBe("New folder");
  });
});

describe("folders accessibility", () => {
  it("has no axe violations with folder tiles, the breadcrumb, search results and the delete dialog", async () => {
    load(project(["med_5"]));
    connect();
    const { container } = view();
    expect((await axe.run(container)).violations).toEqual([]);
    await userEvent.dblClick(tile("Interviews"));
    expect((await axe.run(container)).violations).toEqual([]);
    binStore.getState().setText("a");
    await vi.waitFor(() => expect(optionNames().length).toBeGreaterThan(0));
    expect((await axe.run(container)).violations).toEqual([]);
    binStore.getState().setText("");
    await userEvent.click(screen.getByRole("button", { name: "Media" }));
    await userEvent.click(tile("B-roll"));
    await userEvent.keyboard("{Delete}");
    const dialog = await screen.findByRole("dialog");
    expect((await axe.run(dialog)).violations).toEqual([]);
  });
});
