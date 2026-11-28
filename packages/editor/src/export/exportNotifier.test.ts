// SPDX-License-Identifier: GPL-3.0-or-later
import type { ExportQueueJob, JobId, JobInfo } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { PreferencesClient } from "../prefs/prefsStore.ts";
import type { DesktopBridge } from "../session/desktop.ts";
import { createJobStore } from "../state/jobStore.ts";
import { playExportChime } from "./exportChime.ts";
import { ExportNotifier, MAX_NOTICE_BODY } from "./exportNotifier.ts";
import { createQueueStore } from "./exportQueue.ts";
import { createExportStore } from "./exportStore.ts";

const JOB = "job_1" as JobId;

const job = (patch: Partial<JobInfo>): JobInfo => ({
  id: JOB,
  kind: "export",
  label: "export",
  media_id: null,
  state: "running",
  progress: { fraction: 0.5, message: null },
  cancel_requested: false,
  error: null,
  error_code: null,
  ...patch,
});

function setup(options: { desktop?: boolean } = {}) {
  const jobs = createJobStore();
  const queue = createQueueStore();
  const state = createExportStore();
  const prefs = new PreferencesClient();
  const exportFinished = vi.fn(async (_notice: { title: string; body: string }) => true);
  const chime = vi.fn();
  const desktop =
    options.desktop === false
      ? null
      : ({
          dialog: { openProjectFolder: async () => null },
          notify: { exportFinished },
        } satisfies DesktopBridge);
  queue.getState().set({
    status: "ready",
    jobs: [{ id: JOB, path: "C:\\out\\Film.mp4" } as ExportQueueJob],
  });
  const notifier = new ExportNotifier({
    jobs,
    queue,
    state,
    t: (key, vars) => (vars === undefined ? key : `${key} ${JSON.stringify(vars)}`),
    desktop,
    prefs,
    chime,
  });
  notifier.attach();
  return { jobs, queue, state, prefs, exportFinished, chime, notifier };
}

describe("ExportNotifier", () => {
  it("announces an export it watched run, once, by its file name", () => {
    const { jobs, exportFinished, chime } = setup();
    jobs.getState().applyProgress(job({ state: "queued" }));
    jobs.getState().applyProgress(job({ state: "running" }));
    expect(exportFinished).not.toHaveBeenCalled();
    jobs.getState().applyProgress(job({ state: "succeeded" }));
    expect(exportFinished).toHaveBeenCalledWith({ title: "export.done.title", body: "Film.mp4" });
    jobs.getState().applyProgress(job({ state: "succeeded" }));
    expect(exportFinished).toHaveBeenCalledTimes(1);
    // The sound is off unless the person turns it on.
    expect(chime).not.toHaveBeenCalled();
  });

  it("says why a failed export failed, within what main takes", () => {
    const { jobs, exportFinished } = setup();
    jobs.getState().applyProgress(job({}));
    jobs.getState().applyProgress(job({ state: "failed", error: "the disk is full" }));
    expect(exportFinished).toHaveBeenCalledWith({
      title: "export.failed.title",
      body: 'export.notify.failed {"name":"Film.mp4","message":"the disk is full"}',
    });
    jobs.getState().applyProgress(job({ id: "job_2" as JobId }));
    jobs
      .getState()
      .applyProgress(job({ id: "job_2" as JobId, state: "failed", error: "x".repeat(900) }));
    const body = exportFinished.mock.calls.at(-1)?.[0].body ?? "";
    expect(body.length).toBe(MAX_NOTICE_BODY);
    expect(body.endsWith("…")).toBe(true);
  });

  it("does not announce a cancelled export, one that was already over, or other jobs", () => {
    const { jobs, exportFinished, chime, prefs } = setup();
    void prefs.set("notify.sound", true);
    jobs.getState().applyProgress(job({}));
    jobs.getState().applyProgress(job({ state: "cancelled" }));
    jobs.getState().applyProgress(job({ id: "job_2" as JobId, state: "succeeded" }));
    jobs.getState().applyProgress(job({ id: "job_3" as JobId, kind: "proxy" }));
    jobs.getState().applyProgress(job({ id: "job_3" as JobId, kind: "proxy", state: "succeeded" }));
    expect(exportFinished).not.toHaveBeenCalled();
    expect(chime).not.toHaveBeenCalled();
  });

  it("forgets a running export the engine no longer lists (it restarted)", () => {
    const { jobs, exportFinished } = setup();
    jobs.getState().applyProgress(job({}));
    jobs.getState().reset([]);
    jobs.getState().reset([job({ state: "succeeded" })]);
    expect(exportFinished).not.toHaveBeenCalled();
  });

  it("plays the sound when Settings > Notifications asks for it, also without the desktop app", async () => {
    const { jobs, chime, prefs, exportFinished } = setup({ desktop: false });
    await prefs.set("notify.sound", true);
    jobs.getState().applyProgress(job({}));
    jobs.getState().applyProgress(job({ state: "succeeded" }));
    expect(chime).toHaveBeenCalledTimes(1);
    expect(exportFinished).not.toHaveBeenCalled();
  });

  it("names the file of an export this window started when the queue has not listed it yet", () => {
    const { jobs, queue, state, exportFinished } = setup();
    queue.getState().set({ jobs: [] });
    state.getState().setActive({
      files: [{ jobId: JOB, path: "C:\\out\\Now.mov", fileName: "Now.mov" }],
      seen: true,
      warnings: [],
      estimatedKib: null,
    });
    jobs.getState().applyProgress(job({}));
    jobs.getState().applyProgress(job({ state: "succeeded" }));
    expect(exportFinished).toHaveBeenCalledWith({ title: "export.done.title", body: "Now.mov" });
  });
});

describe("playExportChime", () => {
  it("plays two short notes and closes the audio context after the last", () => {
    const tones: { frequency: { value: number }; onended: (() => void) | null }[] = [];
    const close = vi.fn(async () => undefined);
    class FakeContext {
      currentTime = 1;
      destination = {};
      close = close;
      createOscillator() {
        const tone = {
          type: "",
          frequency: { value: 0 },
          onended: null as (() => void) | null,
          connect: vi.fn(),
          start: vi.fn(),
          stop: vi.fn(),
        };
        tones.push(tone);
        return tone;
      }
      createGain() {
        return {
          gain: { setValueAtTime: vi.fn(), exponentialRampToValueAtTime: vi.fn() },
          connect: vi.fn(),
        };
      }
    }
    playExportChime(FakeContext as unknown as typeof AudioContext);
    expect(tones.map((t) => t.frequency.value)).toEqual([880, 1174.66]);
    expect(tones[0]?.onended).toBeNull();
    tones[1]?.onended?.();
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("is silent where the page has no audio", () => {
    expect(() => playExportChime(undefined)).not.toThrow();
    class Broken {
      constructor() {
        throw new Error("no audio device");
      }
    }
    expect(() => playExportChime(Broken as unknown as typeof AudioContext)).not.toThrow();
  });
});
