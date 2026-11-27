// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The tag editor, the rating stars and the note field (WP-2F.06b): what they show for one asset
// and for several, and that each change is exactly one engine command.
import type { MediaAsset, Project } from "@cutweave/protocol";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { Fragment, createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { MediaOrganize } from "../inspector/MediaOrganize.tsx";
import { menuDialogStore } from "../menus/dialogs.ts";
import { MenuDialogs } from "../menus/MenuDialogs.tsx";
import { docStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { openRatingDialog } from "./RatingDialog.tsx";
import { openTagEditor, TagEditor } from "./TagEditor.tsx";

const F = 705_600_000;

const asset = (
  id: string,
  tags: string[] = [],
  rating: number | null = null,
  note: string | null = null,
) => ({
  id,
  kind: "video",
  path_abs: `/media/${id}.mp4`,
  path_rel: null,
  name: null,
  folder: null,
  ...(tags.length > 0 && { tags }),
  ...(rating !== null && { rating }),
  ...(note !== null && { note }),
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

/** Three tagged clips and one without tags; "Zoo" is only on the fourth's neighbour. */
const DEMO = [
  asset("med_1", ["b-roll", "Interview", "keep"], 4, "Second take is the good one."),
  asset("med_2", ["b-roll", "interview"]),
  asset("med_3", ["b-roll", "night"], 2),
  asset("med_4", ["Zoo"]),
];

const load = (p: Project) =>
  docStore.setState({ status: "ready", rev: 1, committedRev: 1, project: p });
const mediaOf = (id: string): MediaAsset =>
  docStore.getState().project?.media[id as never] as MediaAsset;

let call: ReturnType<typeof vi.fn>;

beforeEach(() => {
  call = vi.fn(async () => ({ changed: [] }));
  engineStore.setState({ rpc: { call } as never, connection: "ready" });
  load(project(DEMO));
});

afterEach(() => {
  cleanup();
  menuDialogStore.getState().close();
  engineStore.setState(initialEngineState);
  viewStore.setState(initialViewState);
  toastStore.getState().clear();
});

const wrap = (...children: ReturnType<typeof h>[]) =>
  render(h(I18nextProvider, { i18n }, h(Fragment, null, ...children)));

const field = () => screen.getByRole("combobox", { name: "Add tag" }) as HTMLInputElement;
const chips = () => {
  const list = screen.queryByRole("list", { name: "Tags" });
  return list === null
    ? []
    : within(list)
        .getAllByRole("listitem")
        .map((li) => `${li.getAttribute("data-tag")}${li.dataset.partial ? "?" : ""}`);
};

describe("the tag editor for one asset", () => {
  it("shows its tags as chips and adds what is typed with one media.add_tags", async () => {
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: ["med_1"] }));
    expect(chips()).toEqual(["b-roll", "Interview", "keep"]);
    await user.type(field(), "  sunset {Enter}");
    expect(call).toHaveBeenCalledExactlyOnceWith(
      "media.add_tags",
      { media: ["med_1"], tags: ["sunset"] },
      undefined,
    );
    expect(field().value).toBe("");
  });

  it("adds on a comma too, and several pasted tags as one step", async () => {
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: ["med_2"] }));
    await user.type(field(), "wide shot,");
    expect(call).toHaveBeenLastCalledWith(
      "media.add_tags",
      { media: ["med_2"], tags: ["wide shot"] },
      undefined,
    );
    fireEvent.paste(field(), { clipboardData: { getData: () => "day 1, Day 1 ,\nsunrise" } });
    expect(call).toHaveBeenLastCalledWith(
      "media.add_tags",
      { media: ["med_2"], tags: ["day 1", "sunrise"] },
      undefined,
    );
    expect(call).toHaveBeenCalledTimes(2);
  });

  it("sends nothing for a tag the asset has already, whatever its case or accents", async () => {
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: ["med_1"] }));
    await user.type(field(), "INTERVIEW{Enter}");
    expect(call).not.toHaveBeenCalled();
    expect(field().value).toBe("");
  });

  it("says why a text cannot be a tag and sends nothing", async () => {
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: ["med_1"] }));
    await user.click(field());
    await user.paste("x".repeat(65));
    await user.keyboard("{Enter}");
    expect(screen.getByRole("alert").textContent).toBe("Use 64 characters or fewer.");
    expect(field().getAttribute("aria-invalid")).toBe("true");
    expect(call).not.toHaveBeenCalled();
    // Typing again takes the message away.
    await user.keyboard("{Backspace}");
    expect(screen.queryByRole("alert")).toBeNull();
    await user.keyboard("{Enter}");
    expect(call).toHaveBeenCalledTimes(1);
  });

  it("refuses a 65th tag before the engine has to", async () => {
    const many = Array.from({ length: 64 }, (_, i) => `t${String(i).padStart(2, "0")}`);
    load(project([asset("med_1", many)]));
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: ["med_1"] }));
    await user.type(field(), "one more{Enter}");
    expect(screen.getByRole("alert").textContent).toBe("An item holds 64 tags at most.");
    expect(call).not.toHaveBeenCalled();
  });

  it("removes a tag with its chip's button and with Backspace in the empty field", async () => {
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: ["med_1"] }));
    await user.click(screen.getByRole("button", { name: "Remove tag Interview" }));
    expect(call).toHaveBeenLastCalledWith(
      "media.remove_tags",
      { media: ["med_1"], tags: ["Interview"] },
      undefined,
    );
    await user.click(field());
    await user.keyboard("{Backspace}");
    expect(call).toHaveBeenLastCalledWith(
      "media.remove_tags",
      { media: ["med_1"], tags: ["keep"] },
      undefined,
    );
    expect(call).toHaveBeenCalledTimes(2);
  });

  it("suggests the project's tags the asset does not have, and takes one on click", async () => {
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: ["med_2"] }));
    await user.click(field());
    const names = () =>
      within(screen.getByRole("listbox", { name: "Suggested tags" }))
        .getAllByRole("option")
        .map((o) => o.textContent);
    // Without b-roll and interview, which it has.
    expect(names().sort()).toEqual(["Zoo", "keep", "night"]);
    await user.keyboard("N");
    expect(names()).toEqual(["night"]);
    await user.click(screen.getByRole("option", { name: "night" }));
    expect(call).toHaveBeenCalledExactlyOnceWith(
      "media.add_tags",
      { media: ["med_2"], tags: ["night"] },
      undefined,
    );
  });

  it("shows the engine's own message when it refuses", async () => {
    call.mockRejectedValueOnce(new Error("an asset has at most 64 tags"));
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: ["med_1"] }));
    await user.type(field(), "x{Enter}");
    await vi.waitFor(() => expect(toastStore.getState().notices).toHaveLength(1));
    expect(toastStore.getState().notices[0]).toMatchObject({
      tone: "danger",
      title: "Could not add the tag",
    });
  });

  it("is disabled while the engine is away", () => {
    engineStore.setState({ connection: "connecting" as never });
    wrap(h(TagEditor, { media: ["med_1"] }));
    expect(field().disabled).toBe(true);
  });
});

describe("the tag editor for several assets", () => {
  const three = ["med_1", "med_2", "med_3"];

  it("shows which tags all of them have and which only some have", () => {
    wrap(h(TagEditor, { media: three }));
    expect(chips()).toEqual(["b-roll", "Interview?", "keep?", "night?"]);
    const list = screen.getByRole("list", { name: "Tags" });
    expect(within(list).getAllByRole("listitem")[1]?.textContent).toContain("2 of 3");
    expect(within(list).getAllByRole("listitem")[2]?.textContent).toContain("1 of 3");
  });

  it("adds a typed tag to all of them as one command", async () => {
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: three }));
    await user.type(field(), "day 2{Enter}");
    expect(call).toHaveBeenCalledExactlyOnceWith(
      "media.add_tags",
      { media: three, tags: ["day 2"] },
      undefined,
    );
  });

  it("adds a tag only some have to the rest, by its button or by typing it", async () => {
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: three }));
    await user.click(screen.getByRole("button", { name: "Add keep to all 3" }));
    expect(call).toHaveBeenLastCalledWith(
      "media.add_tags",
      { media: three, tags: ["keep"] },
      undefined,
    );
    await user.type(field(), "night{Enter}");
    expect(call).toHaveBeenLastCalledWith(
      "media.add_tags",
      { media: three, tags: ["night"] },
      undefined,
    );
    // The tag all three have is not sent again.
    await user.type(field(), "B-Roll{Enter}");
    expect(call).toHaveBeenCalledTimes(2);
  });

  it("removes a tag from all of them as one command", async () => {
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: three }));
    await user.click(screen.getByRole("button", { name: "Remove tag Interview" }));
    expect(call).toHaveBeenCalledExactlyOnceWith(
      "media.remove_tags",
      { media: three, tags: ["Interview"] },
      undefined,
    );
  });

  it("still offers a tag that only some have as a suggestion", async () => {
    const user = userEvent.setup();
    wrap(h(TagEditor, { media: three }));
    await user.type(field(), "ke");
    expect(screen.getByRole("option", { name: "keep" })).toBeTruthy();
    expect(screen.queryByRole("option", { name: "b-roll" })).toBeNull();
  });
});

describe("the Add tag… dialog", () => {
  it("names the asset, or says how many, starts in the field and closes with Done", async () => {
    const user = userEvent.setup();
    wrap(h(MenuDialogs));
    openTagEditor(["med_1"]);
    const one = await screen.findByRole("dialog", { name: "Tags of med_1.mp4" });
    expect(document.activeElement).toBe(within(one).getByRole("combobox", { name: "Add tag" }));
    await user.click(within(one).getByRole("button", { name: "Done" }));
    expect(menuDialogStore.getState().dialog).toBeNull();

    openTagEditor(["med_1", "med_2", "med_3"]);
    const many = await screen.findByRole("dialog", { name: "Tags of 3 items" });
    expect(within(many).getByText(/A dashed tag is on only some of them/)).toBeTruthy();
  });

  it("Enter in the field adds a tag and keeps the dialog open", async () => {
    const user = userEvent.setup();
    wrap(h(MenuDialogs));
    openTagEditor(["med_2", "med_3"]);
    await screen.findByRole("dialog");
    await user.keyboard("sunset{Enter}");
    expect(call).toHaveBeenCalledExactlyOnceWith(
      "media.add_tags",
      { media: ["med_2", "med_3"], tags: ["sunset"] },
      undefined,
    );
    expect(menuDialogStore.getState().dialog).not.toBeNull();
  });

  it("opens for the cards selected in the bin when none are named, and not for none", () => {
    openTagEditor();
    expect(menuDialogStore.getState().dialog).toBeNull();
    viewStore.getState().setSelection([
      { kind: "media", id: "med_2" },
      { kind: "media", id: "med_4" },
    ] as never);
    openTagEditor();
    expect(menuDialogStore.getState().dialog).toEqual({ kind: "tags", media: ["med_2", "med_4"] });
  });

  it("has no axe violations with chips, partial chips, suggestions and an error", async () => {
    const user = userEvent.setup();
    wrap(h(MenuDialogs));
    openTagEditor(["med_1", "med_2", "med_3"]);
    await screen.findByRole("dialog");
    const check = async () => {
      const results = await axe.run(document.body, {
        rules: { "color-contrast": { enabled: false } },
      });
      expect(results.violations.map((v) => `${v.id}: ${v.nodes[0]?.html}`)).toEqual([]);
    };
    await check();
    await user.keyboard("z{ArrowDown}");
    expect(screen.getByRole("option", { name: "Zoo" }).getAttribute("aria-selected")).toBe("true");
    await check();
    await user.paste("x".repeat(70));
    await user.keyboard("{Enter}");
    expect(screen.getByRole("alert")).toBeTruthy();
    await check();
  });
});

describe("Set rating from the palette", () => {
  it("shows the shared rating, rates every selected card with one command, and says when they differ", async () => {
    const user = userEvent.setup();
    wrap(h(MenuDialogs));
    openRatingDialog();
    expect(menuDialogStore.getState().dialog).toBeNull();
    viewStore.getState().setSelection([
      { kind: "media", id: "med_1" },
      { kind: "media", id: "med_3" },
    ] as never);
    openRatingDialog();
    const dialog = await screen.findByRole("dialog", { name: "Rating of 2 items" });
    expect(within(dialog).getByText("The selected items have different ratings.")).toBeTruthy();
    for (const radio of within(dialog).getAllByRole("radio")) {
      expect(radio.getAttribute("aria-checked")).toBe("false");
    }
    await user.click(within(dialog).getByRole("radio", { name: "5 stars" }));
    expect(call).toHaveBeenCalledExactlyOnceWith(
      "media.set_rating",
      { media: ["med_1", "med_3"], rating: 5 },
      undefined,
    );
    const results = await axe.run(document.body, {
      rules: { "color-contrast": { enabled: false } },
    });
    expect(results.violations.map((v) => `${v.id}: ${v.nodes[0]?.html}`)).toEqual([]);
  });
});

describe("the media info's rating, tags and note", () => {
  const info = (id: string) => wrap(h(MediaOrganize, { media: mediaOf(id) }));
  const note = () => screen.getByRole("textbox", { name: "Note" }) as HTMLTextAreaElement;

  it("shows the rating as five stars; a click sets it and the current star clears it", async () => {
    const user = userEvent.setup();
    info("med_1");
    const group = screen.getByRole("radiogroup", { name: "Rating" });
    expect(within(group).getByRole("radio", { name: "4 stars" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    await user.click(within(group).getByRole("radio", { name: "2 stars" }));
    expect(call).toHaveBeenLastCalledWith(
      "media.set_rating",
      { media: ["med_1"], rating: 2 },
      undefined,
    );
    await user.click(within(group).getByRole("radio", { name: "4 stars" }));
    expect(call).toHaveBeenLastCalledWith(
      "media.set_rating",
      { media: ["med_1"], rating: null },
      undefined,
    );
  });

  it("sets the rating with the arrow keys", async () => {
    const user = userEvent.setup();
    info("med_2");
    await user.click(screen.getByRole("radio", { name: "1 star" }));
    await user.keyboard("{ArrowRight}");
    // The document has not changed under the test, so the stars still read "unrated".
    expect(call.mock.calls.map((c) => c[1])).toEqual([
      { media: ["med_2"], rating: 1 },
      { media: ["med_2"], rating: 1 },
    ]);
  });

  it("has the tag editor of the asset", async () => {
    const user = userEvent.setup();
    info("med_3");
    expect(chips()).toEqual(["b-roll", "night"]);
    await user.type(field(), "keep{Enter}");
    expect(call).toHaveBeenCalledExactlyOnceWith(
      "media.add_tags",
      { media: ["med_3"], tags: ["keep"] },
      undefined,
    );
  });

  it("commits the note when the focus leaves it, as one media.set_note", async () => {
    const user = userEvent.setup();
    info("med_2");
    expect(note().value).toBe("");
    await user.type(note(), "Sound starts late.{Enter}Use the second half.");
    expect(call).not.toHaveBeenCalled();
    await user.tab();
    expect(call).toHaveBeenCalledExactlyOnceWith("media.set_note", {
      media: "med_2",
      note: "Sound starts late.\nUse the second half.",
    });
  });

  it("commits on Ctrl+Enter, and not twice when the focus then leaves", async () => {
    const user = userEvent.setup();
    info("med_1");
    expect(note().value).toBe("Second take is the good one.");
    await user.type(note(), " Keep.");
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(call).toHaveBeenCalledExactlyOnceWith("media.set_note", {
      media: "med_1",
      note: "Second take is the good one. Keep.",
    });
    // Leaving the field before the document's patch arrives sends nothing more.
    await user.tab();
    expect(call).toHaveBeenCalledTimes(1);
  });

  it("sends nothing for an unchanged note, removes a cleared one, and Escape puts it back", async () => {
    const user = userEvent.setup();
    info("med_1");
    await user.click(note());
    await user.tab();
    expect(call).not.toHaveBeenCalled();
    await user.type(note(), " and more");
    await user.keyboard("{Escape}");
    expect(note().value).toBe("Second take is the good one.");
    await user.tab();
    expect(call).not.toHaveBeenCalled();
    await user.clear(note());
    await user.tab();
    expect(call).toHaveBeenCalledExactlyOnceWith("media.set_note", { media: "med_1", note: null });
  });

  it("shows the count near the limit and does not send a note that is too long", async () => {
    const user = userEvent.setup();
    info("med_2");
    expect(screen.queryByRole("status")).toBeNull();
    await user.click(note());
    await user.paste("x".repeat(1800));
    expect(screen.getByRole("status").textContent).toBe("1800 of 2,000");
    await user.paste("y".repeat(201));
    expect(screen.getByRole("status").textContent).toBe("A note holds 2,000 characters at most.");
    expect(note().getAttribute("aria-invalid")).toBe("true");
    await user.tab();
    expect(call).not.toHaveBeenCalled();
  });

  it("has no axe violations", async () => {
    const { container } = info("med_1");
    const results = await axe.run(container, {
      rules: { "color-contrast": { enabled: false } },
    });
    expect(results.violations.map((v) => `${v.id}: ${v.nodes[0]?.html}`)).toEqual([]);
  });
});
