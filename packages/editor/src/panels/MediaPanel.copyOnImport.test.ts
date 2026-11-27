// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Copy on import (WP-2F.04c): with "Copy imported files into the project folder" on, every
// import the editor starts asks the engine to copy (`copy_to_project`): the Import button, its
// key, Import folder…, files dropped from the OS and the Import items of the bin's menu. Off
// (the default) sends nothing. The engine's side is in
// `crates/cutweave-engine/tests/copy_on_import.rs`.

import { defaultPreferences } from "@cutweave/preferences";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { binStore, initialBinState } from "../media/binStore.ts";
import { importMedia } from "../media/import.ts";
import { mediaStore } from "../media/mediaStore.ts";
import { placeholderThumbnails, setThumbnailSource } from "../media/thumbnails.ts";
import { UI_COMMANDS } from "../menus/surfaces/uiCommands.ts";
import { prefsStore } from "../prefs/prefsStore.ts";
import { FEATURE_PAGES } from "../settings/featurePages.tsx";
import { docStore } from "../state/docStore.ts";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { MediaPanel } from "./MediaPanel.tsx";

const output = () => ({
  media: [],
  media_ids: [],
  failed: [],
  job_ids: [],
  skipped: { unsupported: 0, unreadable: 0, too_deep: 0 },
  folders: 0,
});

let call: ReturnType<typeof vi.fn>;

function connect(): void {
  call = vi.fn(async () => output());
  engineStore.setState({
    rpc: { call, on: vi.fn(() => () => {}) } as never,
    connection: "ready",
  });
}

function bridge(): void {
  (globalThis as { cutweave?: unknown }).cutweave = {
    dialog: {
      pickMediaFiles: vi.fn(async () => ["/x/a.mp4"]),
      pickMediaFolders: vi.fn(async () => ["/media/Shoot"]),
    },
    files: { pathForFile: (f: File) => `/dropped/${f.name}` },
  };
}

function setCopy(on: boolean): void {
  prefsStore.setState({
    values: { ...prefsStore.getState().values, "media.copy_on_import": on },
  });
}

const view = () => render(h(I18nextProvider, { i18n }, h(MediaPanel)));
const uiCommand = (id: string) => {
  const command = UI_COMMANDS.find((c) => c.id === id);
  if (command === undefined) throw new Error(`no UI command ${id}`);
  return command.run(undefined, { target: "bin" as never, ids: [], call: vi.fn() });
};

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
  jobStore.getState().reset([]);
  mediaStore.getState().reset();
  binStore.setState({ ...initialBinState });
  toastStore.getState().clear();
  setThumbnailSource(placeholderThumbnails);
  prefsStore.setState({
    values: { ...prefsStore.getState().values, "media.keep_structure": true },
  });
  connect();
  bridge();
  setCopy(true);
});
afterEach(() => {
  cleanup();
  setCopy(false);
  delete (globalThis as { cutweave?: unknown }).cutweave;
});

describe("the setting", () => {
  it("is off by default and has a row on the Media and import page", () => {
    expect(defaultPreferences()["media.copy_on_import"]).toBe(false);
    const row = FEATURE_PAGES.find((p) => p.id === "media")?.rows.find(
      (r) => r.id === "media.copy_on_import",
    );
    expect(row?.control).toEqual({ kind: "switch" });
    expect(i18n.t(row?.label as never)).toBe("Copy imported files into the project folder");
  });
});

describe("with the setting on, every import asks for a copy", () => {
  it("the Import button", async () => {
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/x/a.mp4"],
        copy_to_project: true,
      }),
    );
  });

  it("Ctrl+I", async () => {
    view();
    fireEvent.keyDown(window, { key: "i", ctrlKey: true });
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/x/a.mp4"],
        copy_to_project: true,
      }),
    );
  });

  it("Import folder…", async () => {
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import folder…" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/media/Shoot"],
        copy_to_project: true,
      }),
    );
  });

  it("files dropped from the OS", async () => {
    view();
    const region = screen.getByRole("region", { name: "Media" });
    const dataTransfer = { types: ["Files"], files: [new File([], "clip.mp4")], dropEffect: "" };
    fireEvent.drop(region, { dataTransfer });
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/dropped/clip.mp4"],
        copy_to_project: true,
      }),
    );
  });

  it("the Import and Import folder… items of the bin's menu", async () => {
    await uiCommand("bin.import");
    expect(call).toHaveBeenLastCalledWith("media.import", {
      paths: ["/x/a.mp4"],
      copy_to_project: true,
    });
    await uiCommand("bin.import_folder");
    expect(call).toHaveBeenLastCalledWith("media.import", {
      paths: ["/media/Shoot"],
      copy_to_project: true,
    });
  });
});

describe("with the setting off", () => {
  it("nothing about copying is sent", async () => {
    setCopy(false);
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(call).toHaveBeenCalledWith("media.import", { paths: ["/x/a.mp4"] }));
    await uiCommand("bin.import_folder");
    expect(call).toHaveBeenLastCalledWith("media.import", { paths: ["/media/Shoot"] });
  });
});

describe("a project that was never saved", () => {
  it("shows the engine's refusal, which says to save the project first", async () => {
    const { RpcError } = await import("@cutweave/protocol");
    const refusal =
      "this project has not been saved yet, so it has no folder to copy files into; save the project first, or import without copying";
    call.mockRejectedValueOnce(new RpcError("invalid", refusal, "/copy_to_project"));
    const summary = await importMedia(
      {
        engine: engineStore,
        toasts: toastStore,
        t: (key: string, vars?: Record<string, string>) =>
          String(i18n.t(key as never, vars as never)),
      } as never,
      ["/x/a.mp4"],
      null,
      { copyToProject: true },
    );
    expect(summary).toBeNull();
    const notice = toastStore.getState().notices.find((n) => n.tone === "danger");
    expect(notice?.message).toContain("save the project first");
  });
});
