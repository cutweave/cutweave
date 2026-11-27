// SPDX-License-Identifier: GPL-3.0-or-later
import {
  type ExportPresetSpec,
  type JobInfo,
  type Project,
  RpcError,
} from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { createEngineStore } from "../engine/engineStore.ts";
import { PreferencesClient } from "../prefs/prefsStore.ts";
import type { DesktopBridge } from "../session/desktop.ts";
import { createSessionStore } from "../session/sessionStore.ts";
import { createJobStore } from "../state/jobStore.ts";
import { createToastStore } from "../state/toastStore.ts";
import { ENCODERS, H264_OPTIONS, PRESET_ENTRIES } from "../testing/exportFixtures.ts";
import { ExportController, type ExportRequest, exportSource } from "./exportController.ts";
import { createExportStore } from "./exportStore.ts";

const JOB = "job_1" as JobInfo["id"];
const JOB2 = "job_2" as JobInfo["id"];

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

const started = (path = "C:\\p\\film.mp4") => ({
  job_id: JOB,
  path,
  warnings: [],
  estimated_kib: null,
});

function setup(options: { items?: boolean; desktop?: DesktopBridge | null } = {}) {
  const prefs = new PreferencesClient();
  const engine = createEngineStore();
  const showQueue = vi.fn();
  const call = vi.fn(async (name: string, _input: unknown): Promise<unknown> => {
    if (name === "export.start" || name === "export.enqueue") return started();
    if (name === "job.cancel") return job({ cancel_requested: true });
    if (name === "export.list_presets") return { presets: PRESET_ENTRIES, problems: [] };
    if (name === "export.list_encoders") return { codecs: ENCODERS };
    if (name === "export.describe_options") return { schema: H264_OPTIONS };
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
    showQueue,
    now: () => new Date(2026, 10, 26),
  });
  controller.attach();
  return { controller, engine, call, session, jobs, state, toasts, prefs, showQueue };
}

const request: ExportRequest = {
  path: "C:\\p\\film.mp4",
  preset: "youtube-1080p",
  presetId: "youtube-1080p",
  hw: "auto",
};

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

  it("opens and asks for the presets and the encoders of each codec", async () => {
    const { controller, state, call } = setup();
    controller.open();
    expect(state.getState().dialogOpen).toBe(true);
    await vi.waitFor(() => expect(state.getState().encoders.status).toBe("ready"));
    await vi.waitFor(() => expect(state.getState().presets.status).toBe("ready"));
    expect(call).toHaveBeenCalledWith("export.list_presets", {});
    expect(call).toHaveBeenCalledWith("export.list_encoders", {});
  });

  it("copes with an engine that cannot list encoders or presets", async () => {
    const { controller, state, call } = setup();
    call.mockRejectedValueOnce(new RpcError("not_found", "no such method"));
    await controller.loadEncoders();
    expect(state.getState().encoders.status).toBe("unavailable");
    call.mockRejectedValueOnce(new RpcError("not_found", "no such method"));
    await controller.loadPresets();
    expect(state.getState().presets.status).toBe("unavailable");
  });

  it("resolves a preset for the sequence and passes a refusal's pointer on", async () => {
    const { controller, call } = setup();
    call.mockResolvedValueOnce({ preset: "tiktok", width: 608, height: 1080, warnings: [] });
    const ok = await controller.resolve("tiktok", { kind: "sequence" }, null);
    expect(ok).toMatchObject({ ok: true, value: { width: 608 } });
    expect(call).toHaveBeenLastCalledWith("export.resolve_preset", {
      preset: "tiktok",
      sequence: "seq_1",
      range: { kind: "sequence" },
      overrides: null,
    });
    call.mockRejectedValueOnce(new RpcError("invalid", "no in and out points", "/range/kind"));
    expect(await controller.resolve("tiktok", { kind: "in_out" }, null)).toEqual({
      ok: false,
      message: "no in and out points",
      pointer: "/range/kind",
    });
  });

  it("asks for a codec's encoder options once", async () => {
    const { controller, call } = setup();
    expect(await controller.describeOptions("h264", "mp4")).toEqual(H264_OPTIONS);
    expect(await controller.describeOptions("h264", "mp4")).toEqual(H264_OPTIONS);
    expect(call.mock.calls.filter(([name]) => name === "export.describe_options")).toHaveLength(1);
  });

  it("proposes <project folder>/<pattern>.<the preset's extension>", async () => {
    const { controller, prefs } = setup();
    expect(controller.defaultPath()).toBe("C:\\p\\Film.mp4");
    expect(controller.defaultPath("ProRes 422 HQ", "mov")).toBe("C:\\p\\Film.mov");
    await prefs.set("export.filename", "{project}_{sequence}_{preset}_{date}");
    expect(controller.defaultPath("TikTok", "mp4")).toBe("C:\\p\\Film_Main_TikTok_2026-11-26.mp4");
  });

  it("starts at the default preset, else the last one used (Settings > Export)", async () => {
    const { controller, prefs } = setup();
    expect(controller.formDefaults()).toEqual({
      quality: "medium",
      hardware: true,
      presets: [""],
      fileName: "{project}",
    });
    await prefs.set("export.last_preset", "tiktok");
    expect(controller.formDefaults().presets).toEqual(["tiktok"]);
    await prefs.set("export.preset", "youtube-4k");
    await prefs.set("export.quality", "low");
    await prefs.set("export.hw", "off");
    expect(controller.formDefaults()).toMatchObject({
      quality: "low",
      hardware: false,
      presets: ["youtube-4k"],
    });
  });

  it("starts the export, remembers the preset and follows its job to the toast", async () => {
    const reveal = vi.fn(async () => undefined);
    const { controller, state, call, jobs, toasts, prefs } = setup({
      desktop: {
        dialog: { openProjectFolder: async () => null },
        shell: { showItemInFolder: reveal },
      },
    });
    controller.open();
    const outcome = await controller.start({
      ...request,
      collision: "rename",
      range: { kind: "custom", in: "1s", out: "4s" },
      advanced: { audio: { channels: "mono" } },
    });
    expect(outcome).toMatchObject({ kind: "started", files: [{ fileName: "film.mp4" }] });
    expect(call).toHaveBeenCalledWith("export.start", {
      sequence: "seq_1",
      path: "C:\\p\\film.mp4",
      preset: "youtube-1080p",
      hw: "auto",
      collision: "rename",
      range: { kind: "custom", in: "1s", out: "4s" },
      advanced: { audio: { channels: "mono" } },
    });
    expect(prefs.get("export.last_preset")).toBe("youtube-1080p");
    expect(state.getState().active?.files[0]?.fileName).toBe("film.mp4");

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

  it("follows every file of a marker set and reports them together", async () => {
    const { controller, call, state, jobs, toasts } = setup();
    call.mockResolvedValueOnce({
      ...started("C:\\p\\Film_Intro_1.mp4"),
      jobs: [
        { job_id: JOB, path: "C:\\p\\Film_Intro_1.mp4" },
        { job_id: JOB2, path: "C:\\p\\Film_Hook_2.mp4" },
      ],
    });
    await controller.start({ ...request, range: { kind: "markers" } });
    expect(state.getState().active?.files.map((f) => f.jobId)).toEqual([JOB, JOB2]);
    jobs.getState().applyProgress(job({ state: "succeeded" }));
    jobs.getState().applyProgress(job({ id: JOB2 }));
    expect(state.getState().active).not.toBeNull();
    jobs.getState().applyProgress(job({ id: JOB2, state: "succeeded" }));
    expect(state.getState().active).toBeNull();
    expect(toasts.getState().notices[0]?.message).toBe(
      'export.done.files {"count":"2","name":"Film_Intro_1.mp4"}',
    );
  });

  it("adds to the queue without following it, with Show queue on the toast", async () => {
    const { controller, call, state, toasts, showQueue } = setup();
    call.mockResolvedValueOnce({ ...started(), estimated_kib: 51_200 });
    const outcome = await controller.start(request, "queue");
    expect(outcome.kind).toBe("started");
    expect(call).toHaveBeenCalledWith(
      "export.enqueue",
      expect.objectContaining({ path: request.path }),
    );
    expect(state.getState().active).toBeNull();
    const [notice] = toasts.getState().notices;
    expect(notice?.title).toBe("export.queued.title");
    expect(notice?.message).toBe('export.queued.sized {"name":"film.mp4","size":"53 MB"}');
    notice?.actions?.[0]?.onClick();
    expect(showQueue).toHaveBeenCalled();
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

  it("asks before replacing a file, then replaces it", async () => {
    const { controller, call } = setup();
    call.mockRejectedValueOnce(new RpcError("invalid", "exists", "/overwrite"));
    expect(await controller.start(request)).toEqual({ kind: "exists" });
    expect(call).toHaveBeenLastCalledWith(
      "export.start",
      expect.objectContaining({ overwrite: false }),
    );
    expect((await controller.start({ ...request, collision: "overwrite" })).kind).toBe("started");
    expect(call).toHaveBeenLastCalledWith(
      "export.start",
      expect.objectContaining({ collision: "overwrite" }),
    );
  });

  it("says which media files are missing, and exports anyway with allow_offline", async () => {
    const { controller, call, state } = setup();
    call.mockRejectedValueOnce(
      new RpcError(
        "invalid",
        "invalid value at `/media/med_2`: 2 media files are missing: `a.mp4`, `b.wav`; relink " +
          "them (media.relink_folder, media.relink) or pass `allow_offline: true` to export a " +
          '"media offline" slate in their place',
        "/media/med_2",
      ),
    );
    expect(await controller.start(request)).toEqual({
      kind: "offline",
      count: 2,
      names: ["a.mp4", "b.wav"],
    });
    expect(state.getState().active).toBeNull();
    // Not sent unless asked for: the engine's default refuses.
    expect(call.mock.calls.at(-1)?.[1]).not.toHaveProperty("allow_offline");

    expect((await controller.start({ ...request, allowOffline: true })).kind).toBe("started");
    expect(call).toHaveBeenLastCalledWith(
      "export.start",
      expect.objectContaining({ allow_offline: true }),
    );
    // The queue takes the same choice.
    await controller.start({ ...request, allowOffline: true }, "queue");
    expect(call).toHaveBeenLastCalledWith(
      "export.enqueue",
      expect.objectContaining({ allow_offline: true }),
    );
  });

  it("passes the engine's refusal on, with the field it names", async () => {
    const { controller, call, state } = setup();
    call.mockRejectedValueOnce(
      new RpcError("invalid", "CBR needs software; use VBR", "/advanced/video/rate_control/mode"),
    );
    expect(await controller.start(request)).toEqual({
      kind: "refused",
      message: "CBR needs software; use VBR",
      pointer: "/advanced/video/rate_control/mode",
    });
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

  it("proposes the folder Settings > Export names, and the project's until it is known", async () => {
    const { controller, prefs } = setup();
    expect(controller.defaultPath()).toBe("C:\\p\\Film.mp4");
    await prefs.set("export.folder", "last");
    expect(controller.defaultPath()).toBe("C:\\p\\Film.mp4");
    await prefs.set("export.last_folder", "D:\\Renders");
    expect(controller.defaultPath()).toBe("D:\\Renders\\Film.mp4");
    await prefs.set("export.folder", "chosen");
    expect(controller.defaultPath()).toBe("C:\\p\\Film.mp4");
    await prefs.set("export.folder_path", "E:\\Chosen");
    expect(controller.defaultPath()).toBe("E:\\Chosen\\Film.mp4");
  });

  it("remembers the folder picked in the dialog as the last one and keeps the preset's extension", async () => {
    const choose = vi.fn(async (_: string) => "F:\\Elsewhere\\x.mp4");
    const { controller, prefs } = setup({
      desktop: { dialog: { openProjectFolder: async () => null, chooseExportFile: choose } },
    });
    await prefs.set("export.folder", "chosen");
    await prefs.set("export.folder_path", "E:\\Chosen");
    expect(await controller.chooseFile("")).toBe("F:\\Elsewhere\\x.mp4");
    expect(prefs.get("export.last_folder")).toBe("F:\\Elsewhere");
    expect(prefs.get("export.folder_path")).toBe("E:\\Chosen");
    expect(await controller.chooseFile("F:\\Elsewhere\\x.mov", "mov")).toBe("F:\\Elsewhere\\x.mov");
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

describe("ExportController: presets of the person's", () => {
  const mine = {
    ...(PRESET_ENTRIES[0]?.preset as ExportPresetSpec),
    id: "review",
    name: "Review",
  };

  it("saves a preset with export.save_preset and lists the presets again", async () => {
    const { controller, call, state } = setup();
    call.mockImplementation(async (name: string) => {
      if (name === "export.save_preset") return { id: "review", replaced: false };
      if (name === "export.list_presets") {
        return {
          presets: [...PRESET_ENTRIES, { source: "user", supported: true, preset: mine }],
          problems: [],
        };
      }
      throw new Error(`unexpected ${name}`);
    });
    expect(await controller.savePreset(mine)).toEqual({ ok: true, id: "review", replaced: false });
    expect(call).toHaveBeenCalledWith("export.save_preset", { preset: mine });
    const presets = state.getState().presets;
    expect(presets.status === "ready" && presets.value.some((e) => e.preset.id === "review")).toBe(
      true,
    );
  });

  it("says why the engine refused a preset, and at which field", async () => {
    const { controller, call } = setup();
    call.mockRejectedValue(
      new RpcError("invalid", "the name must be 1..=80 characters", "/name"),
    );
    expect(await controller.savePreset(mine)).toEqual({
      ok: false,
      message: "the name must be 1..=80 characters",
      pointer: "/name",
    });
  });

  it("deletes a preset and forgets it as the last one used", async () => {
    const { controller, call, prefs } = setup();
    await prefs.set("export.last_preset", "review");
    call.mockImplementation(async (name: string) => {
      if (name === "export.delete_preset") return { id: "review" };
      if (name === "export.list_presets") return { presets: PRESET_ENTRIES, problems: [] };
      throw new Error(`unexpected ${name}`);
    });
    expect(await controller.deletePreset("review")).toMatchObject({ ok: true, id: "review" });
    expect(call).toHaveBeenCalledWith("export.delete_preset", { id: "review" });
    await vi.waitFor(() => expect(prefs.get("export.last_preset")).toBe(""));
  });
});
