// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Importing folders with their structure (WP-2F.09): the Import folder… button, its key, OS
// folders dropped on the bin, the setting that turns the structure off, and the progress and
// summary toasts. The engine's side is in `crates/cutweave-engine/tests/import_folders.rs`.
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { binStore, initialBinState } from "../media/binStore.ts";
import { describeSkipped, importMedia } from "../media/import.ts";
import { mediaStore } from "../media/mediaStore.ts";
import { placeholderThumbnails, setThumbnailSource } from "../media/thumbnails.ts";
import { prefsStore } from "../prefs/prefsStore.ts";
import { docStore } from "../state/docStore.ts";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { MediaPanel } from "./MediaPanel.tsx";

const output = (extra: object = {}) => ({
  media: [],
  media_ids: [],
  failed: [],
  job_ids: [],
  skipped: { unsupported: 0, unreadable: 0, too_deep: 0 },
  folders: 0,
  ...extra,
});

const imported = (n: number) =>
  Array.from({ length: n }, (_, i) => ({
    path: `/Shoot/f${i}.mp4`,
    media_id: `med_${i}`,
    kind: "video",
    duplicate: false,
    proxy: "none",
  }));

let call: ReturnType<typeof vi.fn>;
let progress: ((e: { done: number; total: number }) => void) | null = null;

function connect(): void {
  call = vi.fn(async () => output());
  progress = null;
  const on = vi.fn((name: string, handler: (e: never) => void) => {
    if (name === "media.import_progress") progress = handler as never;
    return () => {
      progress = null;
    };
  });
  engineStore.setState({ rpc: { call, on } as never, connection: "ready" });
}

function bridge(extra: object = {}): void {
  (globalThis as { cutweave?: unknown }).cutweave = {
    dialog: {
      pickMediaFiles: vi.fn(async () => ["/x/a.mp4"]),
      pickMediaFolders: vi.fn(async () => ["/media/Shoot", "/media/Extras"]),
    },
    files: { pathForFile: (f: File) => `/dropped/${f.name}` },
    ...extra,
  };
}

const view = () => render(h(I18nextProvider, { i18n }, h(MediaPanel)));
const notices = () => toastStore.getState().notices;

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
  prefsStore.setState({
    values: { ...prefsStore.getState().values, "media.keep_structure": true },
  });
  delete (globalThis as { cutweave?: unknown }).cutweave;
});
afterEach(cleanup);

describe("Import folder…", () => {
  it("opens the folder picker and imports the chosen folders in one call", async () => {
    connect();
    bridge();
    call.mockResolvedValueOnce(
      output({
        media: imported(40),
        media_ids: imported(40).map((m) => m.media_id),
        skipped: { unsupported: 5, unreadable: 0, too_deep: 0 },
        folders: 6,
      }),
    );
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import folder…" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/media/Shoot", "/media/Extras"],
      }),
    );
    await waitFor(() => expect(notices().some((n) => n.tone === "success")).toBe(true));
    expect(notices().find((n) => n.tone === "success")?.message).toBe(
      "Imported 40 files into 6 folders; skipped 5 unsupported.",
    );
  });

  it("Ctrl+Alt+I opens the folder dialog", async () => {
    connect();
    const pick = vi.fn(async () => null);
    bridge({ dialog: { pickMediaFolders: pick } });
    view();
    fireEvent.keyDown(window, { key: "i", ctrlKey: true, altKey: true });
    await waitFor(() => expect(pick).toHaveBeenCalledTimes(1));
  });

  it("sends keep_structure false when the setting is off", async () => {
    connect();
    bridge();
    prefsStore.setState({
      values: { ...prefsStore.getState().values, "media.keep_structure": false },
    });
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import folder…" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/media/Shoot", "/media/Extras"],
        keep_structure: false,
      }),
    );
  });

  it("imports an OS folder dropped on the bin, and says so when it is not the desktop app", async () => {
    connect();
    bridge();
    view();
    const region = screen.getByRole("region", { name: "Media" });
    const dataTransfer = { types: ["Files"], files: [new File([], "Shoot")], dropEffect: "" };
    fireEvent.drop(region, { dataTransfer });
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", { paths: ["/dropped/Shoot"] }),
    );
  });

  it("cannot pick folders without the desktop bridge, and says so", async () => {
    connect();
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import folder…" }));
    expect(notices()[0]?.title).toBe("Importing files needs the Cutweave desktop app.");
    expect(call).not.toHaveBeenCalled();
  });
});

describe("importMedia: progress and the summary", () => {
  const deps = () => ({
    engine: engineStore,
    toasts: toastStore,
    t: (key: string, vars?: Record<string, string>) => String(i18n.t(key as never, vars as never)),
  });

  it("shows how far the engine is while it probes, then replaces the toast with the summary", async () => {
    connect();
    let finish: (v: object) => void = () => {};
    call.mockReturnValueOnce(new Promise((resolve) => (finish = resolve)));
    const pending = importMedia(deps() as never, ["/media/Shoot"], null);
    await waitFor(() => expect(progress).not.toBeNull());
    progress?.({ done: 12, total: 45 });
    expect(notices()[0]?.title).toBe("Importing, 12 of 45 files");
    finish(
      output({
        media: imported(2),
        folders: 1,
        skipped: { unsupported: 0, unreadable: 1, too_deep: 0 },
      }),
    );
    const summary = await pending;
    expect(summary).toMatchObject({ added: 2, folders: 1, skipped: { unreadable: 1 } });
    expect(notices().map((n) => n.tone)).toEqual(["success"]);
    expect(notices()[0]?.message).toBe("Imported 2 files into 1 folder; skipped 1 unreadable.");
  });

  it("keeps the short message for plain files and mentions duplicates after a folder", async () => {
    connect();
    call.mockResolvedValueOnce(output({ media: imported(1) }));
    await importMedia(deps() as never, ["/x/a.mp4"], null);
    expect(notices()[0]?.message).toBe("1 file added.");
    toastStore.getState().clear();
    call.mockResolvedValueOnce(
      output({
        media: [...imported(3), { ...imported(1)[0], duplicate: true, media_id: "med_x" }],
        folders: 2,
      }),
    );
    await importMedia(deps() as never, ["/media/Shoot"], null);
    expect(notices()[0]?.message).toBe(
      "Imported 3 files into 2 folders. 1 was already in the project.",
    );
  });

  it("says what was skipped when a folder held nothing it could import", async () => {
    connect();
    call.mockResolvedValueOnce(output({ skipped: { unsupported: 5, unreadable: 0, too_deep: 2 } }));
    await importMedia(deps() as never, ["/media/Docs"], null);
    expect(notices()[0]).toMatchObject({
      tone: "info",
      title: "Nothing to import",
      message: "Skipped 5 unsupported, 2 nested too deep.",
    });
  });

  it("describes the skipped counts in order, and nothing when there are none", () => {
    const t = deps().t as never;
    expect(describeSkipped({ unsupported: 0, unreadable: 0, tooDeep: 0 }, t)).toBe("");
    expect(describeSkipped({ unsupported: 9, unreadable: 3, tooDeep: 1 }, t)).toBe(
      "9 unsupported, 3 unreadable, 1 nested too deep",
    );
  });
});
