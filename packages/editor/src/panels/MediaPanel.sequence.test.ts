// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Image sequences in the bin (WP-2F.05c, ADR-0035): which picked selections are a numbered run,
// the "Import as an image sequence?" question the Import… button asks for one, the single
// `media.import_sequence` (with the frame rate setting) or `media.import` behind each answer,
// and the toasts. The engine's side is in `crates/cutweave-engine/tests/sequence_ffmpeg.rs`.
// #215: a run dropped from the OS gets the same question, and the "copy imported files into the
// project folder" setting is passed with a sequence too.
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Fragment, createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { binStore, initialBinState } from "../media/binStore.ts";
import { mediaStore } from "../media/mediaStore.ts";
import { SequenceOffer } from "../media/SequenceOffer.tsx";
import { fpsText, runOfSelection, sequenceFps, sequenceOfferStore } from "../media/sequence.ts";
import { placeholderThumbnails, setThumbnailSource } from "../media/thumbnails.ts";
import { UI_COMMANDS } from "../menus/surfaces/uiCommands.ts";
import { prefsStore } from "../prefs/prefsStore.ts";
import { docStore } from "../state/docStore.ts";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { MediaPanel } from "./MediaPanel.tsx";

const shots = (from: number, to: number, dir = "/r") =>
  Array.from(
    { length: to - from + 1 },
    (_, i) => `${dir}/shot_${String(from + i).padStart(4, "0")}.png`,
  );

const sequenceOutput = (extra: object = {}) => ({
  media_id: "med_seq",
  kind: "image_sequence",
  duplicate: false,
  path: "/r/shot_0001.png",
  image_sequence: {
    pattern: "shot_%04d.png",
    first: 1,
    count: 300,
    fps: { num: 30, den: 1 },
    missing: [],
  },
  warnings: [],
  job_ids: [],
  proxy: "none",
  ...extra,
});

const importOutput = (paths: readonly string[]) => ({
  media: paths.map((path, i) => ({
    path,
    media_id: `med_${i}`,
    kind: "image",
    duplicate: false,
    proxy: "none",
  })),
  media_ids: paths.map((_, i) => `med_${i}`),
  failed: [],
  job_ids: [],
  skipped: { unsupported: 0, unreadable: 0, too_deep: 0 },
  folders: 0,
});

let call: ReturnType<typeof vi.fn>;

function connect(answer: (name: string, params: never) => unknown): void {
  call = vi.fn(async (name: string, params: never) => answer(name, params));
  engineStore.setState({
    rpc: { call, on: vi.fn(() => () => {}) } as never,
    connection: "ready",
  });
}

function pick(paths: readonly string[]): void {
  (globalThis as { cutweave?: unknown }).cutweave = {
    dialog: { pickMediaFiles: vi.fn(async () => [...paths]) },
  };
}

/** The desktop's side of a drop from the OS: each file has its path, in `/r`. */
function desktop(): void {
  (globalThis as { cutweave?: unknown }).cutweave = {
    files: { pathForFile: (f: File) => `/r/${f.name}` },
  };
}

/** Drops files with these names from the OS on the bin. */
function dropFiles(names: readonly string[]): void {
  const dataTransfer = {
    types: ["Files"],
    files: names.map((name) => new File([], name)),
    dropEffect: "",
  };
  fireEvent.drop(screen.getByRole("region", { name: "Media" }), { dataTransfer });
}

const setCopy = (on: boolean) =>
  prefsStore.setState({
    values: { ...prefsStore.getState().values, "media.copy_on_import": on },
  });

const view = () =>
  render(h(I18nextProvider, { i18n }, h(Fragment, null, h(MediaPanel), h(SequenceOffer))));
const notices = () => toastStore.getState().notices;
const setFps = (value: string) =>
  prefsStore.setState({
    values: { ...prefsStore.getState().values, "media.image_sequence_fps": value as never },
  });

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  viewStore.setState({ ...initialViewState, layouts: {} });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  engineStore.setState({ rpc: null, connection: "none" });
  jobStore.getState().reset([]);
  mediaStore.getState().reset();
  binStore.setState({ ...initialBinState });
  toastStore.getState().clear();
  setThumbnailSource(placeholderThumbnails);
  setFps("sequence");
  setCopy(false);
  sequenceOfferStore.getState().answer("cancel");
  delete (globalThis as { cutweave?: unknown }).cutweave;
});
afterEach(cleanup);

describe("runOfSelection", () => {
  it("is the run when the files differ only in the number at the end of their names", () => {
    expect(runOfSelection(shots(98, 101))).toEqual({
      path: "/r/shot_0098.png",
      first: "shot_0098.png",
      last: "shot_0101.png",
      count: 4,
    });
    // In any order, with gaps, on Windows paths, and without padding.
    expect(runOfSelection(["/r/f10.jpg", "/r/f8.jpg", "/r/f12.jpg"])).toEqual({
      path: "/r/f8.jpg",
      first: "f8.jpg",
      last: "f12.jpg",
      count: 3,
    });
    expect(runOfSelection(["D:\\r\\a 2.EXR", "D:\\r\\a 1.EXR"])?.path).toBe("D:\\r\\a 1.EXR");
  });

  it("is nothing for one file, mixed names, folders or extensions, repeats and non-pictures", () => {
    expect(runOfSelection([])).toBeNull();
    expect(runOfSelection(["/r/shot_0001.png"])).toBeNull();
    expect(runOfSelection(["/r/shot_0001.png", "/r/cover.png"])).toBeNull();
    expect(runOfSelection(["/r/shot_0001.png", "/r/take_0002.png"])).toBeNull();
    expect(runOfSelection(["/r/shot_0001.png", "/q/shot_0002.png"])).toBeNull();
    expect(runOfSelection(["/r/shot_0001.png", "/r/shot_0002.jpg"])).toBeNull();
    expect(runOfSelection(["/r/shot_01.png", "/r/shot_1.png"])).toBeNull();
    expect(runOfSelection(["/r/take1.mp4", "/r/take2.mp4"])).toBeNull();
  });
});

describe("the frame rate setting", () => {
  it("sends a fraction, or nothing for the sequence's own rate", () => {
    expect(sequenceFps("sequence")).toBeUndefined();
    expect(sequenceFps("24")).toEqual({ num: 24, den: 1 });
    expect(sequenceFps("23.976")).toEqual({ num: 24000, den: 1001 });
    expect(sequenceFps("59.94")).toEqual({ num: 60000, den: 1001 });
    expect(fpsText({ num: 30, den: 1 })).toBe("30");
    expect(fpsText({ num: 24000, den: 1001 })).toBe("23.976");
    expect(fpsText({ num: 30000, den: 1001 })).toBe("29.97");
  });
});

describe("Import… with a numbered run of pictures", () => {
  it("asks, and imports the run as one image sequence", async () => {
    connect(() => sequenceOutput());
    pick(shots(1, 300));
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    const dialog = await screen.findByRole("dialog", { name: "Import as an image sequence?" });
    expect(dialog.textContent).toContain(
      "You chose 300 numbered pictures, shot_0001.png to shot_0300.png.",
    );
    expect(call).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Import as image sequence" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import_sequence", { path: "/r/shot_0001.png" }),
    );
    expect(call).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(notices().some((n) => n.tone === "success")).toBe(true));
    const done = notices().find((n) => n.tone === "success");
    expect(done?.title).toBe("Image sequence imported");
    expect(done?.message).toBe("300 pictures at 30 fps.");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("passes the frame rate of the setting and says how many numbers have no file", async () => {
    setFps("23.976");
    connect(() =>
      sequenceOutput({
        image_sequence: {
          pattern: "shot_%04d.png",
          first: 1,
          count: 300,
          fps: { num: 24000, den: 1001 },
          missing: [120, 121, 250],
        },
      }),
    );
    pick(shots(1, 5));
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await userEvent.click(await screen.findByRole("button", { name: "Import as image sequence" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import_sequence", {
        path: "/r/shot_0001.png",
        fps: { num: 24000, den: 1001 },
      }),
    );
    await waitFor(() => expect(notices().some((n) => n.tone === "warning")).toBe(true));
    expect(notices().find((n) => n.tone === "success")?.message).toBe(
      "297 pictures at 23.976 fps.",
    );
    const gaps = notices().find((n) => n.tone === "warning");
    expect(gaps?.title).toBe("Some pictures are missing");
    expect(gaps?.message).toBe(
      "3 numbers in the run have no file. Each shows the picture before it.",
    );
  });

  it("imports separate pictures when asked to", async () => {
    const paths = shots(1, 3);
    connect(() => importOutput(paths));
    pick(paths);
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await userEvent.click(
      await screen.findByRole("button", { name: "Import as separate pictures" }),
    );
    await waitFor(() => expect(call).toHaveBeenCalledWith("media.import", { paths }));
    expect(call).toHaveBeenCalledTimes(1);
  });

  it("imports nothing on Cancel", async () => {
    connect(() => sequenceOutput());
    pick(shots(1, 3));
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(call).not.toHaveBeenCalled();
    expect(notices()).toEqual([]);
  });

  it("does not ask for files that are not a run", async () => {
    const paths = ["/r/shot_0001.png", "/r/cover.png"];
    connect(() => importOutput(paths));
    pick(paths);
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(call).toHaveBeenCalledWith("media.import", { paths }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says why the engine refused", async () => {
    connect(() => {
      throw new Error("the run cannot be imported: no readable file");
    });
    pick(shots(1, 2));
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await userEvent.click(await screen.findByRole("button", { name: "Import as image sequence" }));
    await waitFor(() => expect(notices().some((n) => n.tone === "danger")).toBe(true));
    const failed = notices().find((n) => n.tone === "danger");
    expect(failed?.title).toBe("The image sequence was not imported");
    expect(failed?.message).toContain("no readable file");
  });
});

describe("a numbered run of pictures dropped from the OS", () => {
  const names = ["shot_0002.png", "shot_0001.png", "shot_0003.png"];

  it("asks like the Import… picker, and imports the run as one image sequence", async () => {
    connect(() => sequenceOutput());
    desktop();
    view();
    dropFiles(names);
    const dialog = await screen.findByRole("dialog", { name: "Import as an image sequence?" });
    expect(dialog.textContent).toContain(
      "You chose 3 numbered pictures, shot_0001.png to shot_0003.png.",
    );
    expect(call).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Import as image sequence" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import_sequence", { path: "/r/shot_0001.png" }),
    );
    expect(call).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("imports separate pictures when asked to, and nothing on Cancel", async () => {
    const paths = names.map((name) => `/r/${name}`);
    connect(() => importOutput(paths));
    desktop();
    view();
    dropFiles(names);
    await userEvent.click(
      await screen.findByRole("button", { name: "Import as separate pictures" }),
    );
    await waitFor(() => expect(call).toHaveBeenCalledWith("media.import", { paths }));
    expect(call).toHaveBeenCalledTimes(1);

    dropFiles(names);
    await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(call).toHaveBeenCalledTimes(1);
  });

  it("does not ask for one picture or for files that are not a run", async () => {
    const paths = ["/r/shot_0001.png", "/r/clip.mp4"];
    connect(() => importOutput(paths));
    desktop();
    view();
    dropFiles(["shot_0001.png", "clip.mp4"]);
    await waitFor(() => expect(call).toHaveBeenCalledWith("media.import", { paths }));
    dropFiles(["shot_0001.png"]);
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", { paths: ["/r/shot_0001.png"] }),
    );
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("with the copy on import setting on", () => {
  it("a picked run is copied into the project with its sequence import", async () => {
    setCopy(true);
    connect(() => sequenceOutput());
    pick(shots(1, 3));
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await userEvent.click(await screen.findByRole("button", { name: "Import as image sequence" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import_sequence", {
        path: "/r/shot_0001.png",
        copy_to_project: true,
      }),
    );
  });

  it("a dropped run too, and so are separate pictures", async () => {
    setCopy(true);
    connect(() => sequenceOutput());
    desktop();
    view();
    dropFiles(["a1.png", "a2.png"]);
    await userEvent.click(await screen.findByRole("button", { name: "Import as image sequence" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import_sequence", {
        path: "/r/a1.png",
        copy_to_project: true,
      }),
    );
    connect(() => importOutput(["/r/a1.png", "/r/a2.png"]));
    dropFiles(["a1.png", "a2.png"]);
    await userEvent.click(
      await screen.findByRole("button", { name: "Import as separate pictures" }),
    );
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/r/a1.png", "/r/a2.png"],
        copy_to_project: true,
      }),
    );
  });

  it("the bin menu's Import item passes it with a run", async () => {
    setCopy(true);
    connect(() => sequenceOutput());
    pick(shots(1, 2));
    view();
    const command = UI_COMMANDS.find((c) => c.id === "bin.import");
    const done = command?.run(undefined, { target: "bin" as never, ids: [], call: vi.fn() });
    await userEvent.click(await screen.findByRole("button", { name: "Import as image sequence" }));
    await done;
    expect(call).toHaveBeenCalledWith("media.import_sequence", {
      path: "/r/shot_0001.png",
      copy_to_project: true,
    });
  });
});
