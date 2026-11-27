// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Tags and ratings in the bin (WP-2F.06b): the Filters menu's Tags and Rating, their chips, what
// a card shows, the bin's view state, the Undo words and the palette's two actions.
import type { Project } from "@cutweave/protocol";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { Fragment, createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import {
  activeFilterCount,
  BIN_STORAGE_KEY,
  binStore,
  createBinStore,
  initialBinState,
  NO_FILTERS,
  toMediaFilter,
} from "../media/binStore.ts";
import { computeBinView } from "../media/binView.ts";
import { layoutStore } from "../media/layoutStore.ts";
import { mediaStore } from "../media/mediaStore.ts";
import { placeholderThumbnails, setThumbnailSource } from "../media/thumbnails.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { MenuDialogs } from "../menus/MenuDialogs.tsx";
import { UI_COMMANDS } from "../menus/surfaces/uiCommands.ts";
import { paletteAction } from "../palette/actions.ts";
import { registerBuiltinPaletteActions } from "../palette/builtinActions.ts";
import { historyLabel, undoText } from "../session/historyLabels.ts";
import { docStore } from "../state/docStore.ts";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { translator } from "../timeline/errors.ts";
import { MediaPanel } from "./MediaPanel.tsx";

const F = 705_600_000;

const asset = (id: string, name: string, tags: string[] = [], rating: number | null = null) => ({
  id,
  kind: "video",
  path_abs: `/media/${name}`,
  path_rel: null,
  name: null,
  folder: null,
  ...(tags.length > 0 && { tags }),
  ...(rating !== null && { rating }),
  probe: {
    streams: [{ index: 0, kind: "video", codec: "h264", width: 1920, height: 1080 }],
    duration: 10 * F,
    fps: { num: 30, den: 1 },
  },
});

const project = (media: ReturnType<typeof asset>[]): Project =>
  ({
    meta: { name: "P" },
    settings: {},
    media: Object.fromEntries(media.map((m) => [m.id, m])),
    folders: {},
    sequences: {},
    custom: {},
  }) as unknown as Project;

const DEMO = [
  asset("med_1", "a.mp4", ["b-roll", "Interview", "keep", "night", "wide"], 5),
  asset("med_2", "b.mp4", ["b-roll", "interview"], 3),
  asset("med_3", "c.mp4", ["b-roll"]),
  asset("med_4", "d.mp4", [], 4),
  asset("med_5", "e.mp4"),
];

const load = (p: Project) =>
  docStore.setState({ status: "ready", rev: 1, committedRev: 1, project: p });

const view = () =>
  render(h(I18nextProvider, { i18n }, h(Fragment, null, h(MediaPanel), h(MenuDialogs))));
const cards = () => within(screen.getByRole("listbox")).getAllByRole("option");
const names = () => cards().map((o) => o.querySelector(".cw-media__name")?.textContent);
const card = (name: string) => {
  const found = cards().find((o) => o.querySelector(".cw-media__name")?.textContent === name);
  if (found === undefined) throw new Error(`no card ${name}`);
  return found;
};

/** Opens a submenu of the open Filters menu with the keyboard, as a person without a mouse does. */
async function openSubmenu(user: ReturnType<typeof userEvent.setup>, name: string) {
  const trigger = await screen.findByRole("menuitem", { name });
  trigger.focus();
  await user.keyboard("{ArrowRight}");
}

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
  engineStore.setState({ rpc: null, connection: "none" });
  jobStore.getState().reset([]);
  mediaStore.getState().reset();
  binStore.setState({ ...initialBinState, sort: { by: "imported", order: "asc" } });
  layoutStore.getState().set("grid");
  toastStore.getState().clear();
  setThumbnailSource(placeholderThumbnails);
  load(project(DEMO));
});
afterEach(() => {
  cleanup();
  menuDialogStore.getState().close();
});

describe("the bin's tag and rating filters (view state)", () => {
  it("toggles tags ignoring case and accents and counts each as a filter", () => {
    const store = createBinStore();
    store.getState().toggleTag("Café");
    store.getState().toggleTag("b-roll");
    expect(store.getState().filters.tags).toEqual(["Café", "b-roll"]);
    expect(activeFilterCount(store.getState().filters)).toBe(2);
    store.getState().toggleTag("CAFE");
    expect(store.getState().filters.tags).toEqual(["b-roll"]);
    store.getState().setMinRating(4);
    expect(activeFilterCount(store.getState().filters)).toBe(2);
    expect(toMediaFilter({ text: "", filters: store.getState().filters })).toEqual({
      tags: ["b-roll"],
      min_rating: 4,
    });
    store.getState().clearFilters();
    expect(store.getState().filters).toEqual(NO_FILTERS);
  });

  it("takes at most the 16 tags the engine's filter does", () => {
    const store = createBinStore();
    for (let i = 0; i < 20; i++) store.getState().toggleTag(`tag ${i}`);
    expect(store.getState().filters.tags).toHaveLength(16);
    // One of them can still be taken out.
    store.getState().toggleTag("tag 3");
    expect(store.getState().filters.tags).toHaveLength(15);
  });

  it("saves the minimum rating and never the tags, which are one project's words", () => {
    const data = new Map<string, string>();
    const storage = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, v),
      removeItem: (k: string) => void data.delete(k),
    };
    const first = createBinStore(storage);
    first.getState().toggleTag("interview");
    first.getState().setMinRating(3);
    expect(JSON.parse(data.get(BIN_STORAGE_KEY) as string).state.filters.tags).toEqual([]);
    const second = createBinStore(storage);
    expect(second.getState().filters.tags).toEqual([]);
    expect(second.getState().filters.minRating).toBe(3);

    for (const bad of [0, 6, 2.5, "4", null]) {
      data.set(
        BIN_STORAGE_KEY,
        JSON.stringify({ state: { filters: { minRating: bad, tags: ["x"] } }, version: 1 }),
      );
      expect(createBinStore(storage).getState().filters).toEqual(NO_FILTERS);
    }
  });

  it("forgets the tag filter with the search text when another project opens", () => {
    const store = createBinStore();
    store.getState().setText("beach");
    store.getState().toggleTag("interview");
    store.getState().setMinRating(2);
    store.getState().toggleKind("video");
    store.getState().clearSearch();
    expect(store.getState().text).toBe("");
    expect(store.getState().filters).toEqual({ ...NO_FILTERS, kinds: ["video"], minRating: 2 });
  });

  it("narrows the bin to the assets that have all the tags and at least the rating", () => {
    const p = project(DEMO);
    const ids = (tags: string[], minRating: number | null, text = "") =>
      computeBinView(
        p,
        {
          text,
          filters: { ...NO_FILTERS, tags, minRating },
          sort: { by: "imported", order: "asc" },
          scope: "all",
        },
        null,
      ).cards.map((c) => c.id);
    expect(ids(["B-ROLL"], null)).toEqual(["med_1", "med_2", "med_3"]);
    expect(ids(["b-roll", "interview"], null)).toEqual(["med_1", "med_2"]);
    expect(ids(["b-roll", "interview"], 4)).toEqual(["med_1"]);
    expect(ids([], 4)).toEqual(["med_1", "med_4"]);
    expect(ids(["zoo"], null)).toEqual([]);
    // The search text reads tags too.
    expect(ids([], null, "nigh")).toEqual(["med_1"]);
  });
});

describe("the Filters menu: Tags and Rating", () => {
  it("lists the project's tags; checking some keeps the assets that have all of them", async () => {
    const user = userEvent.setup();
    view();
    await user.click(screen.getByRole("button", { name: "Filters" }));
    await openSubmenu(user, "Tags");
    const boxes = await screen.findAllByRole("menuitemcheckbox");
    expect(boxes.map((b) => b.textContent).slice(-5)).toEqual([
      "b-roll",
      "Interview",
      "keep",
      "night",
      "wide",
    ]);
    await user.click(screen.getByRole("menuitemcheckbox", { name: "b-roll" }));
    // The submenu stays open for the next tag.
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Interview" }));
    expect(
      screen.getByRole("menuitemcheckbox", { name: "Interview" }).getAttribute("aria-checked"),
    ).toBe("true");
    await user.keyboard("{Escape}{Escape}");
    expect(names()).toEqual(["a.mp4", "b.mp4"]);
    expect(screen.getByRole("button", { name: "Filters, 2 active" })).toBeTruthy();
    const chips = screen.getByRole("group", { name: "Active filters" });
    expect(within(chips).getByText("Tag: b-roll")).toBeTruthy();
    expect(within(chips).getByText("Tag: Interview")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toBe("2 of 5");

    await user.click(screen.getByRole("button", { name: "Remove filter Tag: Interview" }));
    expect(names()).toEqual(["a.mp4", "b.mp4", "c.mp4"]);
    await user.click(within(chips).getByRole("button", { name: "Clear all" }));
    expect(names()).toHaveLength(5);
  });

  it("filters by at least N stars, one choice at a time", async () => {
    const user = userEvent.setup();
    view();
    await user.click(screen.getByRole("button", { name: "Filters" }));
    await openSubmenu(user, "Rating");
    const radios = await screen.findAllByRole("menuitemradio");
    expect(radios.map((r) => r.textContent).slice(-6)).toEqual([
      "Any rating",
      "1 star or more",
      "2 stars or more",
      "3 stars or more",
      "4 stars or more",
      "5 stars",
    ]);
    await user.click(screen.getByRole("menuitemradio", { name: "4 stars or more" }));
    await user.keyboard("{Escape}{Escape}");
    expect(names()).toEqual(["a.mp4", "d.mp4"]);
    const chips = screen.getByRole("group", { name: "Active filters" });
    expect(within(chips).getByText("4 stars or more")).toBeTruthy();
    expect(binStore.getState().filters.minRating).toBe(4);

    await user.click(screen.getByRole("button", { name: "Filters, 1 active" }));
    await openSubmenu(user, "Rating");
    await user.click(await screen.findByRole("menuitemradio", { name: "3 stars or more" }));
    await user.keyboard("{Escape}{Escape}");
    expect(names()).toEqual(["a.mp4", "b.mp4", "d.mp4"]);
    await user.click(screen.getByRole("button", { name: "Remove filter 3 stars or more" }));
    expect(names()).toHaveLength(5);
  });

  it("says there are no tags when the project has none", async () => {
    load(project([asset("med_1", "a.mp4"), asset("med_2", "b.mp4")]));
    const user = userEvent.setup();
    view();
    await user.click(screen.getByRole("button", { name: "Filters" }));
    const tags = await screen.findByRole("menuitem", { name: "Tags" });
    expect(tags.getAttribute("aria-disabled")).toBe("true");
  });

  it("the chip menu's Remove filter takes a tag or the rating out", async () => {
    binStore.getState().toggleTag("b-roll");
    binStore.getState().setMinRating(3);
    const remove = UI_COMMANDS.find((c) => c.id === "bin.remove_filter");
    const ctx = { target: { surface: "bin.chip", ids: [] }, ids: [], call: vi.fn() } as never;
    await remove?.run({ chip: "tag-b-roll" }, ctx);
    expect(binStore.getState().filters.tags).toEqual([]);
    await remove?.run({ chip: "rating" }, ctx);
    expect(binStore.getState().filters.minRating).toBeNull();
  });

  it("has no axe violations with tag and rating chips", async () => {
    binStore.getState().toggleTag("b-roll");
    binStore.getState().setMinRating(3);
    const { container } = view();
    expect((await axe.run(container)).violations).toEqual([]);
  });
});

describe("tags and the rating on a card", () => {
  it("shows up to three tags, how many more, and the rating; nothing on a card without them", () => {
    layoutStore.getState().set("list");
    view();
    const a = card("a.mp4");
    const marks = within(a).getByTestId("card-marks");
    expect([...marks.querySelectorAll(".cw-media__tag")].map((t) => t.textContent)).toEqual([
      "b-roll",
      "Interview",
      "keep",
    ]);
    expect(marks.querySelector(".cw-media__tag-more")?.textContent).toBe("+2");
    expect(marks.querySelector(".cw-media__tags")?.getAttribute("title")).toBe(
      "Tags: b-roll, Interview, keep, night, wide",
    );
    const rating = marks.querySelector(".cw-media__rating");
    expect(rating?.textContent).toBe("5");
    expect(rating?.getAttribute("title")).toBe("Rated 5 of 5");
    // Tags without a rating, a rating without tags, neither.
    expect(within(card("c.mp4")).getByTestId("card-marks").querySelector(".cw-media__rating")).toBe(
      null,
    );
    expect(within(card("d.mp4")).getByTestId("card-marks").querySelector(".cw-media__tags")).toBe(
      null,
    );
    expect(within(card("e.mp4")).queryByTestId("card-marks")).toBeNull();
  });

  it("has no axe violations in the list and in the grid", async () => {
    layoutStore.getState().set("list");
    const { container } = view();
    expect((await axe.run(container)).violations).toEqual([]);
    layoutStore.getState().set("grid");
    expect((await axe.run(container)).violations).toEqual([]);
  });
});

describe("Undo in words", () => {
  const t = translator(i18n.t.bind(i18n));

  it("names the tag, rating and note commands", () => {
    expect(historyLabel("media.add_tags", t)).toBe("Add tag");
    expect(historyLabel("media.remove_tags", t)).toBe("Remove tag");
    expect(historyLabel("media.rename_tag", t)).toBe("Rename tag");
    expect(historyLabel("media.set_rating", t)).toBe("Set rating");
    expect(historyLabel("media.set_note", t)).toBe("Edit note");
    expect(
      undoText(
        {
          canUndo: true,
          canRedo: false,
          nextUndo: "media.add_tags",
          nextRedo: null,
          nextUndoActor: "user",
          nextRedoActor: null,
        } as never,
        t,
      ),
    ).toBe("Undo Add tag");
  });
});

describe("the palette's Add tag… and Set rating", () => {
  it("open the tag editor and the stars for the cards selected in the bin", async () => {
    registerBuiltinPaletteActions();
    const addTags = paletteAction("media.add_tags");
    const setRating = paletteAction("media.set_rating");
    expect(addTags).toBeDefined();
    expect(setRating).toBeDefined();
    // Nothing selected: the palette shows why instead of an empty dialog.
    expect(() => addTags?.({})).toThrow("Select media in the bin first.");
    expect(() => setRating?.({})).toThrow("Select media in the bin first.");

    viewStore.getState().setSelection([
      { kind: "media", id: "med_2" },
      { kind: "media", id: "med_3" },
    ] as never);
    await addTags?.({});
    expect(menuDialogStore.getState().dialog).toEqual({ kind: "tags", media: ["med_2", "med_3"] });
    await setRating?.({});
    expect(menuDialogStore.getState().dialog).toEqual({
      kind: "rating",
      media: ["med_2", "med_3"],
    });
  });
});
