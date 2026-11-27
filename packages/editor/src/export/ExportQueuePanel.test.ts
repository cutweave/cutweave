// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2G.04: the export queue panel over the real stores and a recording engine: rows with
// progress and time left, the row actions (icon buttons with tooltips), reordering, the row's
// context menu table, and progress events that re-render the queue panel only.
import type { ExportQueueJob, JobId, JobInfo } from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h, Profiler } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { HistoryPanel } from "../history/HistoryPanel.tsx";
import { i18n } from "../i18n/index.ts";
import { createMenuServices } from "../menus/services.ts";
import { MediaPanel } from "../panels/MediaPanel.tsx";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { ExportQueuePanel } from "./ExportQueuePanel.tsx";
import { moveTarget, queueStore } from "./exportQueue.ts";
import { EXPORT_ROW_SURFACE, registerQueueMenu } from "./queueMenu.ts";
import { exportQueue, resetExportController } from "./useExport.ts";

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
});

const F = 705_600_000;
const id = (n: number) => `job_${n}` as JobId;

function queued(n: number, patch: Partial<ExportQueueJob> = {}): ExportQueueJob {
  return {
    id: id(n),
    state: "queued",
    progress: { fraction: null, message: null },
    cancel_requested: false,
    error: null,
    path: `C:\\out\\clip${n}.mp4`,
    sequence: "seq_1" as ExportQueueJob["sequence"],
    in: 0,
    out: 4 * F,
    width: 1080,
    height: 1920,
    frame_rate: { num: 30, den: 1 },
    frames: 120,
    duration: 4 * F,
    rev: 1 as ExportQueueJob["rev"],
    ...patch,
  };
}

const info = (n: number, patch: Partial<JobInfo>): JobInfo => ({
  id: id(n),
  kind: "export",
  label: "export",
  media_id: null,
  state: "running",
  progress: { fraction: 0, message: null },
  cancel_requested: false,
  error: null,
  ...patch,
});

let jobs: ExportQueueJob[];
let paused: boolean;
let call: ReturnType<typeof vi.fn>;

beforeEach(() => {
  resetExportController();
  jobs = [
    queued(1, {
      state: "running",
      progress: { fraction: 0.25, message: null, fps: 48, eta_ms: 15_000 },
    }),
    queued(2),
    queued(3),
    queued(4, { state: "failed", error: "Encoder: no space left on device" }),
    queued(5, { state: "succeeded", progress: { fraction: 1, message: null } }),
  ];
  paused = false;
  call = vi.fn(async (name: string, input: { id?: JobId; before?: JobId; paused?: boolean }) => {
    const answer = () => ({ paused, parallel: 1, jobs });
    if (name === "export.queue") return answer();
    if (name === "export.pause_queue") {
      paused = input.paused === true;
      return answer();
    }
    if (name === "export.reorder") {
      const moving = jobs.find((j) => j.id === input.id) as ExportQueueJob;
      jobs = jobs.filter((j) => j.id !== input.id);
      const at =
        input.before === undefined ? jobs.length : jobs.findIndex((j) => j.id === input.before);
      jobs.splice(at, 0, moving);
      return answer();
    }
    if (name === "export.remove") {
      jobs = jobs.filter((j) => j.id !== input.id);
      return answer();
    }
    if (name === "export.cancel" || name === "export.retry") {
      const state = name === "export.cancel" ? "cancelled" : "queued";
      jobs = jobs.map((j) => (j.id === input.id ? { ...j, state } : j));
      return jobs.find((j) => j.id === input.id);
    }
    if (name === "edit.history") return { entries: [], next_cursor: null };
    throw new Error(`unexpected ${name}`);
  });
  const on = () => () => undefined;
  engineStore.setState({ ...initialEngineState, rpc: { call, on } as never, connection: "ready" });
  jobStore.setState({ jobs: {} });
  queueStore.setState({ status: "idle", paused: false, jobs: [] });
  toastStore.getState().clear();
});

afterEach(() => {
  cleanup();
  resetExportController();
});

const mount = (...children: ReturnType<typeof h>[]) =>
  render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, ...children)));

const order = () =>
  [...screen.getByRole("list", { name: "Exports" }).querySelectorAll("li[data-job]")].map((li) =>
    li.getAttribute("data-job"),
  );

describe("ExportQueuePanel", () => {
  it("lists the exports with their progress, time left and errors", async () => {
    mount(h(ExportQueuePanel));
    const list = await screen.findByRole("list", { name: "Exports" });
    expect(order()).toEqual(["job_1", "job_2", "job_3", "job_4", "job_5"]);
    const running = within(list).getByRole("listitem", { name: "clip1.mp4, Exporting" });
    expect(within(running).getByText("25%")).toBeTruthy();
    expect(within(running).getByText("1080 × 1920 · 0:04 · 48 fps · about 0:15 left")).toBeTruthy();
    const failed = within(list).getByRole("listitem", { name: "clip4.mp4, Failed" });
    expect(within(failed).getByRole("alert").textContent).toBe(
      "Failed: Encoder: no space left on device",
    );
    expect(screen.getByText("2 waiting")).toBeTruthy();
    expect((await axe.run(screen.getByTestId("export-queue"))).violations).toEqual([]);
  });

  it("acts on a row with icon buttons: move, cancel, retry, remove; pause the queue", async () => {
    const user = userEvent.setup();
    mount(h(ExportQueuePanel));
    const list = await screen.findByRole("list", { name: "Exports" });
    const second = within(list).getByRole("listitem", { name: "clip2.mp4, Waiting" });
    // The first waiting export cannot move up; the second moves above it.
    expect(within(second).queryByRole("button", { name: "Move up" })).toBeNull();
    const third = within(list).getByRole("listitem", { name: "clip3.mp4, Waiting" });
    await user.click(within(third).getByRole("button", { name: "Move up" }));
    await waitFor(() => expect(order()).toEqual(["job_1", "job_3", "job_2", "job_4", "job_5"]));
    expect(call).toHaveBeenCalledWith("export.reorder", { id: id(3), before: id(2) });

    await user.click(
      within(within(list).getByRole("listitem", { name: "clip3.mp4, Waiting" })).getByRole(
        "button",
        { name: "Move down" },
      ),
    );
    expect(call).toHaveBeenLastCalledWith("export.reorder", { id: id(3) });

    await user.click(
      within(within(list).getByRole("listitem", { name: "clip4.mp4, Failed" })).getByRole(
        "button",
        { name: "Retry export" },
      ),
    );
    await waitFor(() => expect(call).toHaveBeenCalledWith("export.retry", { id: id(4) }));
    await user.click(
      within(within(list).getByRole("listitem", { name: "clip1.mp4, Exporting" })).getByRole(
        "button",
        { name: "Cancel export" },
      ),
    );
    await waitFor(() => expect(call).toHaveBeenCalledWith("export.cancel", { id: id(1) }));
    await user.click(
      within(within(list).getByRole("listitem", { name: "clip5.mp4, Done" })).getByRole("button", {
        name: "Remove from list",
      }),
    );
    await waitFor(() => expect(order()).not.toContain("job_5"));

    await user.click(screen.getByRole("button", { name: "Pause the queue" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Resume the queue" })).toBeTruthy(),
    );
    expect(screen.getByText(/^Paused, \d+ waiting$/)).toBeTruthy();
  });

  it("asks the list again when an export job changes state, not on every progress event", async () => {
    mount(h(ExportQueuePanel));
    await screen.findByRole("list", { name: "Exports" });
    const off = exportQueue().attach();
    const asked = () => call.mock.calls.filter(([n]) => n === "export.queue").length;
    const before = asked();
    act(() =>
      jobStore.getState().applyProgress(info(1, { progress: { fraction: 0.3, message: null } })),
    );
    await waitFor(() => expect(asked()).toBe(before + 1));
    for (let i = 0; i < 5; i++) {
      act(() =>
        jobStore
          .getState()
          .applyProgress(info(1, { progress: { fraction: 0.3 + i / 10, message: null } })),
      );
    }
    await new Promise((r) => setTimeout(r, 100));
    expect(asked()).toBe(before + 1);
    act(() => jobStore.getState().applyProgress(info(1, { state: "succeeded" })));
    await waitFor(() => expect(asked()).toBe(before + 2));
    off();
  });

  it("re-renders only the queue panel, at 4 Hz and more, while an export runs", async () => {
    const commits = { queue: 0, history: 0, bin: 0 };
    const onRender = (which: keyof typeof commits) => () => {
      commits[which]++;
    };
    mount(
      h(Profiler, { id: "queue", onRender: onRender("queue") }, h(ExportQueuePanel)),
      h(Profiler, { id: "history", onRender: onRender("history") }, h(HistoryPanel)),
      h(Profiler, { id: "bin", onRender: onRender("bin") }, h(MediaPanel)),
    );
    const list = await screen.findByRole("list", { name: "Exports" });
    await new Promise((r) => setTimeout(r, 50));
    commits.queue = 0;
    commits.history = 0;
    commits.bin = 0;
    // Eight progress events a quarter of a second apart, in two seconds.
    for (let i = 1; i <= 8; i++) {
      const progress = {
        fraction: 0.25 + i / 20,
        message: null,
        fps: 48,
        eta_ms: 15_000 - i * 250,
      };
      act(() => jobStore.getState().applyProgress(info(1, { progress })));
    }
    expect(within(list).getByText("65%")).toBeTruthy();
    expect(commits.queue).toBeGreaterThanOrEqual(8);
    expect(commits.history).toBe(0);
    expect(commits.bin).toBe(0);
  });

  it("offers the row actions on the row's context menu", () => {
    const services = createMenuServices();
    registerQueueMenu(services);
    expect(services.surfaces.get("export.row")).toBe(EXPORT_ROW_SURFACE);
    expect(EXPORT_ROW_SURFACE.rows.map((r) => r.id ?? r.command)).toEqual([
      "export.queue_cancel",
      "export.queue_retry",
      "export.queue_up",
      "export.queue_down",
      "export.queue_open",
      "reveal",
      "copy-error",
      "export.queue_remove",
    ]);
    for (const command of [
      "export.queue_cancel",
      "export.queue_retry",
      "export.queue_up",
      "export.queue_down",
      "export.queue_open",
      "export.queue_remove",
    ]) {
      expect(services.ui.get(command)?.hidden).toBe(true);
    }
    const target = (data: object) => ({
      target: { surface: "export.row", ids: [], data },
      ids: [],
      keys: {},
      playhead: null,
    });
    const failed = target({ id: id(4), path: "C:\\out\\clip4.mp4", state: "failed", error: "x" });
    const enabled = (
      row: (typeof EXPORT_ROW_SURFACE.rows)[number],
      ctx: ReturnType<typeof target>,
    ) => row.enabledIf?.(ctx) ?? true;
    const rows = Object.fromEntries(EXPORT_ROW_SURFACE.rows.map((r) => [r.id ?? r.command, r]));
    expect(enabled(rows["export.queue_retry"] as never, failed)).toBe(true);
    expect(enabled(rows["export.queue_cancel"] as never, failed)).toBe(false);
    expect(enabled(rows["copy-error"] as never, failed)).toBe(true);
    expect(rows["copy-error"]?.args?.(failed)).toEqual({ text: "x" });
    expect(enabled(rows.reveal as never, failed)).toBe(false);
    // Open file: only a finished file opens.
    expect(enabled(rows["export.queue_open"] as never, failed)).toBe(false);
    const done = target({ id: id(5), path: "C:\\out\\clip5.mp4", state: "succeeded", error: null });
    expect(enabled(rows["export.queue_open"] as never, done)).toBe(true);
  });

  it("opens a finished export's file from its row and from the menu, and says when it does not open", async () => {
    const openFile = vi.fn(async (_path: string) => undefined);
    const host = globalThis as { cutweave?: unknown };
    host.cutweave = {
      dialog: { openProjectFolder: async () => null },
      shell: { showItemInFolder: async () => undefined, openFile },
    };
    try {
      resetExportController();
      const user = userEvent.setup();
      mount(h(ExportQueuePanel));
      const done = await screen.findByRole("listitem", { name: "clip5.mp4, Done" });
      await user.click(within(done).getByRole("button", { name: "Open file" }));
      expect(openFile).toHaveBeenCalledWith("C:\\out\\clip5.mp4");
      // Only a finished export has a file to open.
      const failed = screen.getByRole("listitem", { name: "clip4.mp4, Failed" });
      expect(within(failed).queryByRole("button", { name: "Open file" })).toBeNull();

      const services = createMenuServices();
      registerQueueMenu(services);
      const ctx = {
        target: {
          surface: "export.row",
          ids: [],
          data: { id: id(5), path: "C:\\out\\clip5.mp4", state: "succeeded", error: null },
        },
        ids: [],
        keys: {},
        playhead: null,
      };
      openFile.mockRejectedValueOnce(new Error("No application is associated with this file"));
      await services.ui.get("export.queue_open")?.run(undefined, ctx as never);
      expect(openFile).toHaveBeenCalledTimes(2);
      expect(toastStore.getState().notices.at(-1)).toMatchObject({
        tone: "danger",
        title: "The file did not open",
        message: "C:\\out\\clip5.mp4",
      });
    } finally {
      delete host.cutweave;
    }
  });

  it("has no Open file button without the desktop app", async () => {
    mount(h(ExportQueuePanel));
    const done = await screen.findByRole("listitem", { name: "clip5.mp4, Done" });
    expect(within(done).queryByRole("button", { name: "Open file" })).toBeNull();
  });

  it("knows where a waiting export moves", () => {
    const list = [queued(1, { state: "running" }), queued(2), queued(3), queued(4)];
    expect(moveTarget(list, id(2), "up")).toBeUndefined();
    expect(moveTarget(list, id(3), "up")).toBe(id(2));
    expect(moveTarget(list, id(2), "down")).toBe(id(4));
    expect(moveTarget(list, id(3), "down")).toBeNull();
    expect(moveTarget(list, id(4), "down")).toBeUndefined();
    expect(moveTarget(list, id(1), "up")).toBeUndefined();
  });
});
