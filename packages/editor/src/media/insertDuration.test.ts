// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// "Duration…" on an image card (#215; ADR-0035, Decision 1): the dialog keeps a length for the
// picture for this run of the app, and the next insert of that card (Enter, a drop, the menu)
// sends it as `duration` with its `timeline.insert`. Nothing is sent by the dialog itself.
import type { Project, RpcClient } from "@cutweave/protocol";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { MenuDialogs } from "../menus/MenuDialogs.tsx";
import { docStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { translator } from "../timeline/errors.ts";
import { insertAtPlayhead } from "./insert.ts";
import {
  flicksToSeconds,
  insertDurationFor,
  insertDurationStore,
  secondsToFlicks,
} from "./insertDuration.ts";

const F = 705_600_000;
const hex = (prefix: string, n: number) => `${prefix}_${n.toString(16).padStart(32, "0")}`;
const SEQ = hex("seq", 1);
const V1 = hex("trk", 1);
const PHOTO = hex("med", 1);
const VIDEO = hex("med", 2);

const project = (): Project =>
  ({
    meta: { name: "P" },
    settings: { still_duration: 3 * F },
    media: {
      [PHOTO]: { id: PHOTO, kind: "image", path_abs: "/m/photo.png", probe: { streams: [] } },
      [VIDEO]: { id: VIDEO, kind: "video", path_abs: "/m/a.mp4", probe: { streams: [] } },
    },
    sequences: {
      [SEQ]: {
        id: SEQ,
        name: "Main",
        format: { width: 1920, height: 1080, frame_rate: { num: 30, den: 1 } },
        track_order: [V1],
        tracks: { [V1]: { id: V1, kind: "video", name: "V1", items: {}, locked: false } },
        custom: {},
      },
    },
    custom: {},
  }) as unknown as Project;

let call: ReturnType<typeof vi.fn>;
const deps = () => ({
  doc: docStore,
  view: viewStore,
  engine: engineStore,
  toasts: toastStore,
  t: translator(i18n.t.bind(i18n)),
});
const open = () =>
  menuDialogStore.getState().open({ kind: "insert-duration", media: PHOTO, name: "photo.png" });
const chosen = () => insertDurationStore.getState().byMedia.get(PHOTO);

beforeEach(() => {
  docStore.setState({ status: "ready", rev: 1, committedRev: 1, project: project() });
  call = vi.fn(async () => ({ clips: [] }));
  engineStore.setState({ rpc: { call } as unknown as RpcClient, connection: "ready" });
  insertDurationStore.getState().reset();
  menuDialogStore.getState().close();
  toastStore.getState().clear();
  render(h(I18nextProvider, { i18n }, h(MenuDialogs)));
});
afterEach(() => {
  cleanup();
  menuDialogStore.getState().close();
  insertDurationStore.getState().reset();
  engineStore.setState({ rpc: null, connection: "none" });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
});

describe("seconds and flicks", () => {
  it("reads what a person types and shows a length as seconds", () => {
    expect(secondsToFlicks("2.5")).toBe(2.5 * F);
    expect(secondsToFlicks("0,5")).toBe(F / 2);
    for (const bad of ["", "0", "-1", "abc"]) expect(secondsToFlicks(bad), bad).toBeNull();
    expect(flicksToSeconds(2.5 * F)).toBe("2.5");
  });
});

describe("the Duration… dialog", () => {
  it("proposes the project's still duration and keeps what the person sets, sending nothing", async () => {
    open();
    const dialog = await screen.findByRole("dialog", { name: "Duration of photo.png" });
    expect(dialog.textContent).toContain("the next time you put it on the timeline");
    const field = screen.getByLabelText("Seconds") as HTMLInputElement;
    expect(field.value).toBe("3");
    // Nothing chosen yet: there is nothing to go back from.
    expect(screen.queryByRole("button", { name: "Use the project’s duration" })).toBeNull();
    await userEvent.clear(field);
    await userEvent.type(field, "2.5");
    await userEvent.click(screen.getByRole("button", { name: "Set duration" }));
    expect(chosen()).toBe(2.5 * F);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(call).not.toHaveBeenCalled();
  });

  it("refuses what is not a length, and Cancel changes nothing", async () => {
    open();
    const field = (await screen.findByLabelText("Seconds")) as HTMLInputElement;
    await userEvent.clear(field);
    await userEvent.type(field, "soon");
    expect(
      (screen.getByRole("button", { name: "Set duration" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(chosen()).toBeUndefined();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows the choice again and goes back to the project's duration when asked", async () => {
    insertDurationStore.getState().set(PHOTO, 8 * F);
    open();
    const field = (await screen.findByLabelText("Seconds")) as HTMLInputElement;
    expect(field.value).toBe("8");
    await userEvent.click(screen.getByRole("button", { name: "Use the project’s duration" }));
    expect(chosen()).toBeUndefined();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("the next insert of the card", () => {
  it("sends the chosen length as duration, for that picture only", async () => {
    insertDurationStore.getState().set(PHOTO, 2.5 * F);
    viewStore.getState().setPlayhead(F);
    expect(await insertAtPlayhead(deps(), { id: PHOTO, kind: "image" })).toBe(true);
    expect(call).toHaveBeenCalledWith("timeline.insert", {
      sequence: SEQ,
      track: V1,
      media: PHOTO,
      at: F,
      duration: 2.5 * F,
      overlap: "new_track",
    });
    // It lasts for later inserts too.
    await insertAtPlayhead(deps(), { id: PHOTO, kind: "image" });
    expect(call.mock.calls[1]?.[1]).toMatchObject({ duration: 2.5 * F });
    call.mockClear();
    await insertAtPlayhead(deps(), { id: VIDEO, kind: "video" });
    expect(call.mock.calls[0]?.[1]).not.toHaveProperty("duration");
  });

  it("leaves the length to the engine without a choice, and never gives one to what is not a still", async () => {
    await insertAtPlayhead(deps(), { id: PHOTO, kind: "image" });
    expect(call.mock.calls[0]?.[1]).not.toHaveProperty("duration");
    // A choice kept for an id that is a video (it cannot be made from the menu) is not used.
    insertDurationStore.getState().set(VIDEO, F);
    expect(insertDurationFor(docStore.getState().project, VIDEO)).toBeUndefined();
    expect(insertDurationFor(null, PHOTO)).toBeUndefined();
  });
});
