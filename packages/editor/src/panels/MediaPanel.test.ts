// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The media bin (WP-1M.03): cards from the engine document, import through the desktop bridge and
// OS drops, removal with the engine's message, drag sessions for the timeline, keyboard use, axe.
import { type MediaId, type Project, RpcError } from "@cutweave/protocol";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { dragStore, MEDIA_DRAG_TYPE } from "../media/dragStore.ts";
import { mediaStore } from "../media/mediaStore.ts";
import {
  placeholderThumbnails,
  setThumbnailSource,
  type ThumbnailSource,
} from "../media/thumbnails.ts";
import { docStore } from "../state/docStore.ts";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { MediaPanel } from "./MediaPanel.tsx";

const F = 705_600_000;

const stream = (kind: string, codec: string, extra: object = {}) => ({
  index: 0,
  kind,
  codec,
  ...extra,
});

const asset = (
  id: string,
  kind: string,
  name: string,
  streams: object[],
  seconds: number | null,
) => ({
  id,
  kind,
  path_abs: `/media/${name}`,
  path_rel: null,
  probe: {
    streams,
    duration: seconds === null ? null : seconds * F,
    fps: kind === "video" ? { num: 30000, den: 1001 } : null,
  },
});

const MEDIA = {
  med_a: asset(
    "med_a",
    "video",
    "beach.mp4",
    [stream("video", "h264", { width: 1920, height: 1080 })],
    10,
  ),
  med_b: asset("med_b", "audio", "music.wav", [stream("audio", "pcm_s16le")], 95),
  med_c: asset(
    "med_c",
    "image",
    "logo.png",
    [stream("video", "png", { width: 512, height: 512 })],
    null,
  ),
};

const track = (id: string, kind: string, items: object[] = []) => ({
  id,
  kind,
  name: id,
  items: Object.fromEntries(items.map((i) => [(i as { id: string }).id, i])),
  muted: false,
  hidden: false,
  locked: false,
  solo: false,
  magnetic: false,
  gain: 0,
  custom: {},
});

const clipOf = (id: string, media: string) => ({
  kind: "clip",
  id,
  name: null,
  start: 0,
  duration: F,
  enabled: true,
  linked_group: null,
  media,
  source_in: 0,
  audio: { gain: 0 },
  visual: { transform: {}, opacity: 1 },
  custom: {},
});

const project = (items: object[] = []): Project =>
  ({
    meta: { name: "P" },
    settings: {},
    media: structuredClone(MEDIA),
    sequences: {
      seq_1: {
        id: "seq_1",
        name: "Main",
        format: { width: 1920, height: 1080, frame_rate: { num: 30, den: 1 } },
        track_order: ["trk_v", "trk_a"],
        tracks: { trk_v: track("trk_v", "video", items), trk_a: track("trk_a", "audio") },
        custom: {},
      },
    },
    custom: {},
  }) as unknown as Project;

const load = (p: Project | null) =>
  docStore.setState({
    status: p === null ? "empty" : "ready",
    rev: 1,
    committedRev: 1,
    project: p,
  });

let call: ReturnType<typeof vi.fn>;

function connect(): void {
  call = vi.fn(async () => ({}));
  engineStore.setState({ rpc: { call } as never, connection: "ready" });
}

function bridge(extra: object = {}): void {
  (globalThis as { cutweave?: unknown }).cutweave = {
    dialog: { pickMediaFiles: vi.fn(async () => ["/x/a.mp4", "/x/b.mov", "/x/c.wav"]) },
    files: { pathForFile: (f: File) => `/dropped/${f.name}` },
    ...extra,
  };
}

const view = () => render(h(I18nextProvider, { i18n }, h(MediaPanel)));
const notices = () => toastStore.getState().notices;

beforeEach(() => {
  viewStore.setState({ ...initialViewState, layouts: {} });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  engineStore.setState({ rpc: null, connection: "none" });
  jobStore.getState().reset([]);
  mediaStore.getState().reset();
  toastStore.getState().clear();
  dragStore.getState().end();
  setThumbnailSource(placeholderThumbnails);
  delete (globalThis as { cutweave?: unknown }).cutweave;
});
afterEach(cleanup);

describe("MediaPanel cards", () => {
  it("shows the empty state with an Import button before anything is imported", () => {
    connect();
    bridge();
    view();
    expect(screen.getByText("No media yet")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Import" })).toBeTruthy();
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("lists the project's assets with duration, resolution, fps and codec", () => {
    load(project());
    view();
    const list = screen.getByRole("listbox", { name: "Media files" });
    const options = within(list).getAllByRole("option");
    expect(options).toHaveLength(3);
    const beach = options[0] as HTMLElement;
    expect(within(beach).getByText("beach.mp4")).toBeTruthy();
    expect(within(beach).getByText("0:10")).toBeTruthy();
    expect(within(beach).getByText("1920×1080 · 29.97 fps")).toBeTruthy();
    expect(within(beach).getByText("H264")).toBeTruthy();
    expect(within(options[1] as HTMLElement).getByText("1:35")).toBeTruthy();
    // A still has no duration chip.
    expect(within(options[2] as HTMLElement).queryByText(/^\d+:\d\d$/)).toBeNull();
  });

  it("draws the design-system placeholder tile until a thumbnail source says otherwise", () => {
    load(project());
    const { container } = view();
    expect(container.querySelector(".cw-media__thumb-icon")).not.toBeNull();
    expect(container.querySelector("img")).toBeNull();
  });

  it("asks the thumbnail source and falls back to the tile when the image fails", () => {
    load(project());
    const source: ThumbnailSource = {
      thumbnailFor: (r) =>
        r.kind === "video"
          ? { kind: "image", src: `http://127.0.0.1:1/media/${r.id}/thumb?v=${r.indexed}` }
          : { kind: "placeholder" },
    };
    setThumbnailSource(source);
    const { container } = view();
    const img = container.querySelector("img");
    expect(img?.getAttribute("src")).toBe("http://127.0.0.1:1/media/med_a/thumb?v=0");
    fireEvent.error(img as Element);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelectorAll(".cw-media__thumb-icon")).toHaveLength(3);
  });

  it("re-asks the source when media.indexed arrives", () => {
    load(project());
    setThumbnailSource({
      thumbnailFor: (r) =>
        r.indexed > 0
          ? { kind: "image", src: `http://127.0.0.1:1/${r.id}.jpg` }
          : { kind: "placeholder" },
    });
    const { container } = view();
    expect(container.querySelector("img")).toBeNull();
    fireEvent.focus(document.body);
    mediaStore.getState().applyIndexed({ media_id: "med_a" as MediaId });
    return waitFor(() => expect(container.querySelector("img")).not.toBeNull());
  });

  it("badges an asset whose analysis failed, keeping it usable", async () => {
    load(project());
    view();
    mediaStore.getState().applyAnalysisFailed({
      media_id: "med_a" as MediaId,
      stage: "thumbnails",
      job_id: "job_1" as never,
      error: "decoder missing",
    });
    const beach = await screen.findByText("Analysis failed");
    expect(beach.closest("[data-media-id]")?.getAttribute("data-media-id")).toBe("med_a");
    expect(beach.parentElement?.getAttribute("title")).toContain("decoder missing");
  });

  it("badges an asset while its analysis jobs run", () => {
    load(project());
    jobStore.getState().applyProgress({
      id: "job_1",
      kind: "analysis",
      label: "index",
      media_id: "med_b",
      state: "running",
      progress: { fraction: 0.2, message: null },
      cancel_requested: false,
      error: null,
    } as never);
    view();
    expect(screen.getAllByText("Analyzing")).toHaveLength(1);
  });

  it("marks an asset with no readable streams", () => {
    const p = project();
    (p.media as Record<string, unknown>).med_a = asset("med_a", "video", "broken.mp4", [], null);
    load(p);
    view();
    expect(screen.getByText("Unreadable")).toBeTruthy();
  });
});

describe("MediaPanel import", () => {
  it("imports the three files the native dialog returns in one media.import", async () => {
    connect();
    bridge();
    call.mockResolvedValueOnce({
      media: [],
      media_ids: [],
      failed: [],
      job_ids: [],
    });
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/x/a.mp4", "/x/b.mov", "/x/c.wav"],
      }),
    );
    expect(call).toHaveBeenCalledTimes(1);
  });

  it("imports OS-dropped files by their paths and shows the drop target while dragging", async () => {
    connect();
    bridge();
    call.mockResolvedValueOnce({
      media: [
        { path: "/dropped/a.mp4", media_id: "med_a", kind: "video", duplicate: false },
        { path: "/dropped/b.mp4", media_id: "med_b", kind: "video", duplicate: true },
      ],
      media_ids: ["med_a", "med_b"],
      failed: [],
      job_ids: [],
    });
    const { container } = view();
    const region = screen.getByRole("region", { name: "Media" });
    const files = ["a.mp4", "b.mp4", "c.mp4"].map((n) => new File(["x"], n));
    const dataTransfer = { types: ["Files"], files, dropEffect: "" };
    fireEvent.dragEnter(region, { dataTransfer });
    expect(container.querySelector(".cw-media__drop")).not.toBeNull();
    fireEvent.drop(region, { dataTransfer });
    expect(container.querySelector(".cw-media__drop")).toBeNull();
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("media.import", {
        paths: ["/dropped/a.mp4", "/dropped/b.mp4", "/dropped/c.mp4"],
      }),
    );
    await waitFor(() => expect(notices().some((n) => n.tone === "success")).toBe(true));
    const done = notices().find((n) => n.tone === "success");
    expect(done?.message).toBe("1 file added. 1 was already in the project.");
  });

  it("ignores drags that carry no files (a card dragged within the app)", () => {
    connect();
    bridge();
    const { container } = view();
    fireEvent.dragEnter(screen.getByRole("region", { name: "Media" }), {
      dataTransfer: { types: [MEDIA_DRAG_TYPE] },
    });
    expect(container.querySelector(".cw-media__drop")).toBeNull();
  });

  it("turns each failed[] entry into an error toast and adds no card", async () => {
    connect();
    bridge();
    call.mockResolvedValueOnce({
      media: [],
      media_ids: [],
      failed: [{ path: "/x/notes.txt", code: "unsupported", error: "no streams" }],
      job_ids: [],
    });
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(notices().some((n) => n.tone === "danger")).toBe(true));
    const toast = notices().find((n) => n.tone === "danger");
    expect(toast?.title).toBe("notes.txt not imported");
    expect(toast?.message).toMatch(/no video or audio/);
    expect(screen.queryByRole("option")).toBeNull();
    expect(notices().some((n) => n.tone === "success")).toBe(false);
  });

  it("shows the engine's message when the call itself is refused", async () => {
    connect();
    bridge();
    call.mockRejectedValueOnce(new RpcError("invalid", "path is outside the allowed folders"));
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(notices()[0]?.title).toBe("Import failed"));
    expect(notices()[0]?.message).toBe("path is outside the allowed folders");
  });

  it("filters what cannot be a path before it reaches the engine", async () => {
    connect();
    bridge({
      dialog: { pickMediaFiles: vi.fn(async () => ["", "/ok.mp4", "/ok.mp4", "bad\u0000", 7]) },
    });
    call.mockResolvedValueOnce({ media: [], media_ids: [], failed: [], job_ids: [] });
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(call).toHaveBeenCalledWith("media.import", { paths: ["/ok.mp4"] }));
  });

  it("cannot import without the desktop bridge, and says so", async () => {
    connect();
    view();
    await userEvent.click(screen.getByRole("button", { name: "Import" }));
    expect(notices()[0]?.title).toBe("Importing files needs the Cutweave desktop app.");
    expect(call).not.toHaveBeenCalled();
  });

  it("disables Import while no engine is connected", () => {
    bridge();
    view();
    expect((screen.getByRole("button", { name: "Import" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it("Ctrl+I opens the file dialog", async () => {
    connect();
    const pick = vi.fn(async () => null);
    bridge({ dialog: { pickMediaFiles: pick } });
    view();
    fireEvent.keyDown(window, { key: "i", ctrlKey: true });
    await waitFor(() => expect(pick).toHaveBeenCalledTimes(1));
  });
});

describe("MediaPanel removal", () => {
  it("removes an unused asset without asking", async () => {
    connect();
    load(project());
    view();
    await userEvent.click(screen.getByRole("option", { name: /music\.wav/ }));
    await userEvent.click(screen.getByRole("button", { name: "Remove from project" }));
    expect(call).toHaveBeenCalledWith("media.remove", { id: "med_b" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("asks first when clips use the asset, then shows the engine's message when it refuses", async () => {
    connect();
    load(project([clipOf("itm_1", "med_a"), clipOf("itm_2", "med_a")]));
    call.mockRejectedValueOnce(
      new RpcError("invalid", "media med_a is used by 2 clips; delete them first"),
    );
    view();
    await userEvent.click(screen.getByRole("option", { name: /beach\.mp4/ }));
    await userEvent.click(screen.getByRole("button", { name: "Remove from project" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove beach.mp4?" });
    expect(within(dialog).getByText("2 clips on the timeline use it.")).toBeTruthy();
    expect(call).not.toHaveBeenCalled();
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(call).toHaveBeenCalledWith("media.remove", { id: "med_a" }));
    await waitFor(() => expect(notices()[0]?.title).toBe("beach.mp4 not removed"));
    expect(notices()[0]?.message).toBe("media med_a is used by 2 clips; delete them first");
  });

  it("Cancel leaves a used asset alone", async () => {
    connect();
    load(project([clipOf("itm_1", "med_a")]));
    view();
    await userEvent.click(screen.getByRole("option", { name: /beach\.mp4/ }));
    await userEvent.keyboard("{Delete}");
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(call).not.toHaveBeenCalled();
  });
});

describe("MediaPanel dragging and keyboard", () => {
  it("starts a drag session with the media id and kind for the timeline", () => {
    load(project());
    view();
    const card = screen.getByRole("option", { name: /beach\.mp4/ });
    const data = new Map<string, string>();
    const dataTransfer = {
      setData: (k: string, v: string) => data.set(k, v),
      effectAllowed: "",
    };
    fireEvent.dragStart(card, { dataTransfer });
    expect(data.get(MEDIA_DRAG_TYPE)).toBe("med_a");
    expect(dragStore.getState().session).toEqual({ id: "med_a", kind: "video", duration: 10 * F });
    fireEvent.dragEnd(card);
    expect(dragStore.getState().session).toBeNull();
  });

  it("a still drags without a duration", () => {
    load(project());
    view();
    fireEvent.dragStart(screen.getByRole("option", { name: /logo\.png/ }), {
      dataTransfer: { setData() {}, effectAllowed: "" },
    });
    expect(dragStore.getState().session).toEqual({ id: "med_c", kind: "image", duration: null });
  });

  it("arrow keys move focus and selection through the grid", async () => {
    load(project());
    view();
    const options = screen.getAllByRole("option");
    options[0]?.focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(screen.getAllByRole("option")[1]);
    expect(viewStore.getState().selection).toEqual([{ kind: "media", id: "med_b" }]);
    await userEvent.keyboard("{End}");
    expect(document.activeElement).toBe(screen.getAllByRole("option")[2]);
    await userEvent.keyboard("{Home}");
    expect(document.activeElement).toBe(screen.getAllByRole("option")[0]);
    expect(screen.getAllByRole("option").filter((o) => o.tabIndex === 0)).toHaveLength(1);
  });

  it("the list view is navigable with Up and Down only", async () => {
    load(project());
    view();
    await userEvent.click(screen.getByRole("radio", { name: "List" }));
    expect(screen.getByRole("listbox").getAttribute("data-layout")).toBe("list");
    screen.getAllByRole("option")[0]?.focus();
    await userEvent.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(screen.getAllByRole("option")[1]);
    await userEvent.keyboard("{ArrowUp}{ArrowUp}");
    expect(document.activeElement).toBe(screen.getAllByRole("option")[0]);
    await userEvent.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(screen.getAllByRole("option")[0]);
  });

  it("Enter inserts at the playhead on the selected track, never failing on overlap", async () => {
    connect();
    load(project());
    call.mockResolvedValue({ at: 0, duration: 0, clips: ["itm_9"] });
    viewStore.getState().setPlayhead(2 * F + 1000);
    viewStore.getState().setSelection([{ kind: "track", id: "trk_v" as never }]);
    view();
    screen.getAllByRole("option")[0]?.focus();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(call).toHaveBeenCalledTimes(1));
    expect(call).toHaveBeenCalledWith("timeline.insert", {
      sequence: "seq_1",
      track: "trk_v",
      media: "med_a",
      at: 2 * F,
      overlap: "new_track",
    });
    await waitFor(() =>
      expect(viewStore.getState().selection).toEqual([{ kind: "item", id: "itm_9" }]),
    );
  });

  it("Enter puts sound on the first audio track when no track is selected", async () => {
    connect();
    load(project());
    call.mockResolvedValue({ at: 0, duration: 0, clips: [] });
    view();
    screen.getAllByRole("option")[1]?.focus();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(call).toHaveBeenCalled());
    expect(call.mock.calls[0]?.[1]).toMatchObject({ track: "trk_a", media: "med_b" });
  });

  it("an engine refusal on Enter becomes a toast", async () => {
    connect();
    load(project());
    call.mockRejectedValue(new RpcError("locked", "track is locked"));
    view();
    screen.getAllByRole("option")[0]?.focus();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(notices()[0]?.title).toBe("Clip not added"));
    expect(notices()[0]?.message).toMatch(/locked/);
  });
});

describe("MediaPanel accessibility", () => {
  it("has no axe violations (empty, grid and list)", async () => {
    connect();
    bridge();
    const { container, unmount } = view();
    expect((await axe.run(container)).violations).toEqual([]);
    unmount();
    load(project([clipOf("itm_1", "med_a")]));
    mediaStore.getState().applyAnalysisFailed({
      media_id: "med_b" as MediaId,
      stage: "waveform",
      job_id: "job_1" as never,
      error: "boom",
    });
    const grid = view();
    expect((await axe.run(grid.container)).violations).toEqual([]);
    await userEvent.click(screen.getByRole("radio", { name: "List" }));
    expect((await axe.run(grid.container)).violations).toEqual([]);
  });
});
