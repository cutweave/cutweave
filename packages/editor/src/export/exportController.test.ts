// SPDX-License-Identifier: GPL-3.0-or-later
import { type JobInfo, type Project, RpcError } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { createEngineStore } from "../engine/engineStore.ts";
import { PreferencesClient } from "../prefs/prefsStore.ts";
import type { DesktopBridge } from "../session/desktop.ts";
import { createSessionStore } from "../session/sessionStore.ts";
import { createJobStore } from "../state/jobStore.ts";
import { createToastStore } from "../state/toastStore.ts";
import { ExportController, exportSource } from "./exportController.ts";
import { createExportStore } from "./exportStore.ts";

const JOB = "job_1" as JobInfo["id"];

function project(withItems: boolean): Project {
  return {
    meta: { name: "Film" },
    settings: {},
    media: {},
    sequences: {
      seq_1: {
        id: "seq_1",
        name: "Main",
        format: { width: 1920, height: 1080, frame_rate: { num: 30, den: 1 } },
        tracks: {
          trk_1: { id: "trk_1", items: withItems ? { itm_1: { id: "itm_1" } } : {} },
        },
        track_order: ["trk_1"],
        custom: {},
      },
    },
    custom: {},
  } as unknown as Project;
}

function job(patch: Partial<JobInfo>): JobInfo {
  return {
    id: JOB,
    kind: "export",
    label: "export",
    media_id: null,
    state: "running",
    progress: { fraction: 0.5, message: "frame 5 of 10, 12 fps, about 0:01 left" },
    cancel_requested: false,
    error: null,
    ...patch,
  };
}

function setup(options: { items?: boolean; desktop?: DesktopBridge | null } = {}) {
  const prefs = new PreferencesClient();
  const engine = createEngineStore();
  const call = vi.fn(async (name: string, _input: unknown): Promise<unknown> => {
    if (name === "export.start") {
      return { job_id: JOB, path: "C:\\p\\film.mp4" };
    }
    if (name === "job.cancel") return job({ cancel_requested: true });
    if (name === "media.capabilities") {
      return { hw_encode: [{ implementation: "h264_nvenc" }] };
    }
    throw new Error(`unexpected ${name}`);
  });
  engine.setState({ rpc: { call } as never, connection: "ready" });
  const session = createSessionStore();
  session.setState({ known: true, path: "C:\\p\\film.cutweave", name: "Film" });
  const jobs = createJobStore();
  const state = createExportStore();
  const toasts = createToastStore();
  const doc = project(options.items ?? true);
  const controller = new ExportController({
    engine,
    project: () => doc,
    session,
    jobs,
    state,
    toasts,
    t: (key, vars) => (vars === undefined ? key : `${key} ${JSON.stringify(vars)}`),
    desktop: options.desktop ?? null,
    prefs,
  });
  controller.attach();
  return { controller, engine, call, session, jobs, state, toasts, prefs };
}

const request = {
  path: "C:\\p\\film.mp4",
  preset: "h264-mp4-1080p",
  hw: "auto",
  overwrite: false,
} as const;

describe("exportSource", () => {
  it("knows whether the timeline has clips", () => {
    expect(exportSource(project(true))?.hasItems).toBe(true);
    expect(exportSource(project(false))?.hasItems).toBe(false);
    expect(exportSource(null)).toBeNull();
    expect(exportSource(project(true))?.facts).toEqual({
      width: 1920,
      height: 1080,
      fps: { num: 30, den: 1 },
    });
  });
});

describe("ExportController", () => {
  it("does not open on an empty timeline", () => {
    const { controller, state } = setup({ items: false });
    controller.open();
    expect(state.getState().dialogOpen).toBe(false);
    expect(controller.available()).toBe(false);
  });

  it("opens and asks which hardware encoders there are", async () => {
    const { controller, state, call } = setup();
    controller.open();
    expect(state.getState().dialogOpen).toBe(true);
    await vi.waitFor(() => expect(state.getState().probe.status).toBe("ready"));
    expect(state.getState().probe).toEqual({ status: "ready", encoders: ["h264_nvenc"] });
    expect(call).toHaveBeenCalledWith("media.capabilities", {});
  });

  it("copes with an engine that cannot answer the capabilities query", async () => {
    const { controller, state, call } = setup();
    call.mockRejectedValueOnce(new RpcError("not_found", "no such method"));
    await controller.loadEncoders();
    expect(state.getState().probe.status).toBe("unavailable");
  });

  it("proposes <project folder>/<name>.mp4", () => {
    const { controller } = setup();
    expect(controller.defaultPath()).toBe("C:\\p\\Film.mp4");
  });

  it("starts the export and follows its job to the toast", async () => {
    const reveal = vi.fn(async () => undefined);
    const { controller, state, call, jobs, toasts } = setup({
      desktop: {
        dialog: { openProjectFolder: async () => null },
        shell: { showItemInFolder: reveal },
      },
    });
    controller.open();
    const outcome = await controller.start(request);
    expect(outcome).toEqual({ kind: "started" });
    expect(call).toHaveBeenCalledWith("export.start", {
      sequence: "seq_1",
      path: "C:\\p\\film.mp4",
      preset: "h264-mp4-1080p",
      hw: "auto",
      overwrite: false,
    });
    expect(state.getState().active?.fileName).toBe("film.mp4");

    jobs.getState().applyProgress(job({}));
    expect(state.getState().active?.seen).toBe(true);
    expect(toasts.getState().notices).toEqual([]);

    jobs.getState().applyProgress(
      job({
        state: "succeeded",
        progress: { fraction: 1, message: "exported 10 frames (0.33 s) with h264_nvenc" },
      }),
    );
    expect(state.getState().active).toBeNull();
    expect(state.getState().dialogOpen).toBe(false);
    const [notice] = toasts.getState().notices;
    expect(notice?.tone).toBe("success");
    expect(notice?.message).toBe("film.mp4: exported 10 frames (0.33 s) with h264_nvenc");
    notice?.actions?.[0]?.onClick();
    expect(reveal).toHaveBeenCalledWith("C:\\p\\film.mp4");
  });

  it("settles a job that finished before the start answer arrived", async () => {
    const { controller, state, jobs, toasts } = setup();
    jobs
      .getState()
      .applyProgress(job({ state: "succeeded", progress: { fraction: 1, message: null } }));
    await controller.start(request);
    expect(state.getState().active).toBeNull();
    expect(toasts.getState().notices[0]?.tone).toBe("success");
  });

  it("asks before replacing a file", async () => {
    const { controller, call } = setup();
    call.mockRejectedValueOnce(new RpcError("invalid", "exists", "/overwrite"));
    expect(await controller.start(request)).toEqual({ kind: "exists" });
    expect((await controller.start({ ...request, overwrite: true })).kind).toBe("started");
    expect(call).toHaveBeenLastCalledWith(
      "export.start",
      expect.objectContaining({ overwrite: true }),
    );
  });

  it("passes the engine's refusal on", async () => {
    const { controller, call, state } = setup();
    call.mockRejectedValueOnce(new RpcError("unsupported", "no H.264 encoder"));
    const outcome = await controller.start(request);
    expect(outcome.kind).toBe("refused");
    expect(outcome.kind === "refused" && outcome.message).toContain("no H.264 encoder");
    expect(state.getState().active).toBeNull();
  });

  it("shows the engine's message when the job fails", async () => {
    const { controller, jobs, toasts } = setup();
    await controller.start(request);
    jobs.getState().applyProgress(job({}));
    jobs.getState().applyProgress(job({ state: "failed", error: "disk full" }));
    expect(toasts.getState().notices[0]).toMatchObject({
      tone: "danger",
      title: "export.failed.title",
      message: "disk full",
    });
  });

  it("cancels through job.cancel and says nothing was written", async () => {
    const { controller, state, call, jobs, toasts } = setup();
    await controller.start(request);
    jobs.getState().applyProgress(job({}));
    await controller.cancel();
    expect(call).toHaveBeenCalledWith("job.cancel", { id: JOB });
    expect(state.getState().cancelling).toBe(true);
    jobs.getState().applyProgress(job({ state: "cancelled" }));
    expect(state.getState().active).toBeNull();
    expect(toasts.getState().notices[0]?.title).toBe("export.cancelled.title");
  });

  it("says so when the engine restarts under a running export", async () => {
    const { controller, state, jobs, toasts } = setup();
    await controller.start(request);
    jobs.getState().applyProgress(job({}));
    jobs.getState().reset([]);
    expect(state.getState().active).toBeNull();
    expect(toasts.getState().notices[0]?.title).toBe("export.lost.title");
  });

  it("keeps going when the dialog is hidden", async () => {
    const { controller, state, jobs } = setup();
    controller.open();
    await controller.start(request);
    controller.close();
    jobs.getState().applyProgress(job({}));
    expect(state.getState().dialogOpen).toBe(false);
    expect(state.getState().active).not.toBeNull();
    controller.open();
    expect(state.getState().dialogOpen).toBe(true);
  });

  it("opens on the native Export… command", () => {
    let send: (command: string) => void = () => undefined;
    const { state } = setup({
      desktop: {
        dialog: { openProjectFolder: async () => null },
        menu: {
          onCommand: (listener) => {
            send = listener;
            return () => undefined;
          },
        },
      },
    });
    send("export");
    expect(state.getState().dialogOpen).toBe(true);
  });

  it("starts the form at the person's quality and hardware choice (Settings > Export)", async () => {
    const { controller, prefs } = setup();
    expect(controller.formDefaults()).toEqual({ quality: "medium", hardware: true });
    await prefs.set("export.quality", "low");
    await prefs.set("export.hw", "off");
    expect(controller.formDefaults()).toEqual({ quality: "low", hardware: false });
  });

  it("proposes the folder Settings > Export names, and the project's until it is known", async () => {
    const { controller, prefs } = setup();
    // Project folder is the default.
    expect(controller.defaultPath()).toBe("C:\\p\\Film.mp4");
    // "Last folder" with none yet, and "chosen" with none picked, fall back to the project's.
    await prefs.set("export.folder", "last");
    expect(controller.defaultPath()).toBe("C:\\p\\Film.mp4");
    await prefs.set("export.last_folder", "D:\\Renders");
    expect(controller.defaultPath()).toBe("D:\\Renders\\Film.mp4");
    await prefs.set("export.folder", "chosen");
    expect(controller.defaultPath()).toBe("C:\\p\\Film.mp4");
    await prefs.set("export.folder_path", "E:\\Chosen");
    expect(controller.defaultPath()).toBe("E:\\Chosen\\Film.mp4");
  });

  it("remembers the folder picked in the dialog as the last one and leaves the chosen one alone", async () => {
    const choose = vi.fn(async (_: string) => "F:\\Elsewhere\\x.mp4");
    const { controller, prefs } = setup({
      desktop: { dialog: { openProjectFolder: async () => null, chooseExportFile: choose } },
    });
    await prefs.set("export.folder", "chosen");
    await prefs.set("export.folder_path", "E:\\Chosen");
    expect(await controller.chooseFile("")).toBe("F:\\Elsewhere\\x.mp4");
    expect(prefs.get("export.last_folder")).toBe("F:\\Elsewhere");
    expect(prefs.get("export.folder_path")).toBe("E:\\Chosen");
    expect(prefs.get("export.folder")).toBe("chosen");
  });

  it("does not remember a cancelled save dialog", async () => {
    const { controller, prefs } = setup({
      desktop: {
        dialog: { openProjectFolder: async () => null, chooseExportFile: async () => null },
      },
    });
    expect(await controller.chooseFile("")).toBeNull();
    expect(prefs.get("export.last_folder")).toBe("");
  });

  it("uses the native save dialog when there is one", async () => {
    const choose = vi.fn(async (_: string) => "C:\\out\\x.mp4");
    const { controller } = setup({
      desktop: { dialog: { openProjectFolder: async () => null, chooseExportFile: choose } },
    });
    expect(await controller.chooseFile("")).toBe("C:\\out\\x.mp4");
    expect(choose).toHaveBeenCalledWith("C:\\p\\Film.mp4");
  });
});
