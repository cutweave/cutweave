// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2F.03b: the editor's picture of each video's proxy. The engine is asked when a project
// opens, when the media change, when a proxy job starts or ends and when the window is back; the
// badge on a card follows the job's progress in between; "Regenerate proxies" and "Delete
// proxies" are the catalog commands `media.delete_proxy` and `media.generate_proxy`.
import type { JobId, JobInfo, MediaId, Project, ProxyInfo, RpcClient } from "@cutweave/protocol";
import { RpcError } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import { createJobStore } from "../state/jobStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import {
  createProxyStore,
  deleteProxies,
  proxyBadge,
  refreshProxies,
  regenerateProxies,
  startProxyWiring,
} from "./proxies.ts";

const med = (n: number) => `med_${n}` as MediaId;
const jobId = (n: number) => `job_${n}` as JobId;

const info = (
  n: number,
  status: ProxyInfo["status"],
  more: Partial<ProxyInfo> = {},
): ProxyInfo => ({
  media_id: med(n),
  status,
  wanted: true,
  reason: null,
  profile: "prores_proxy-960",
  path: null,
  bytes: null,
  size: null,
  job_id: null,
  progress: null,
  error: null,
  ...more,
});

const job = (n: number, state: JobInfo["state"], fraction = 0): JobInfo => ({
  id: jobId(n),
  kind: "proxy",
  label: "proxy",
  media_id: med(n),
  state,
  progress: { fraction, message: null } as JobInfo["progress"],
  cancel_requested: false,
  error: state === "failed" ? "disk full" : null,
  error_code: state === "failed" ? "failed" : null,
});

/** An engine that holds `proxies`; generating queues job 9 for each asset, deleting evicts. */
function setup(proxies: ProxyInfo[]) {
  const disk = { proxies, fail: false };
  const call = vi.fn(async (name: string, args: unknown) => {
    const media = (args as { media: MediaId[] | null }).media;
    if (disk.fail && name !== "media.proxy_status") {
      throw new RpcError("unsupported", "this engine makes no proxies");
    }
    if (name === "media.generate_proxy") {
      disk.proxies = disk.proxies.map((p) =>
        media?.includes(p.media_id) && p.status !== "ready"
          ? { ...p, status: "queued" as const, job_id: jobId(9) }
          : p,
      );
    } else if (name === "media.delete_proxy") {
      disk.proxies = disk.proxies.map((p) =>
        media?.includes(p.media_id) ? { ...p, status: "evicted" as const, job_id: null } : p,
      );
    } else if (name !== "media.proxy_status") {
      throw new Error(`unexpected ${name}`);
    }
    return { proxies: disk.proxies, policy: "auto" };
  });
  const engine = createStore<EngineStoreState>()(
    () => ({ rpc: { call } as unknown as RpcClient, connection: "ready" }) as EngineStoreState,
  );
  const pushed: { title: string; message?: string }[] = [];
  const toasts = {
    getState: () => ({ push: (toast: { title: string }) => pushed.push(toast) }),
  } as unknown as import("zustand").StoreApi<ToastStore>;
  const proxyStore = createProxyStore();
  const jobs = createJobStore();
  const deps = { engine, toasts, proxies: proxyStore, t: ((key: string) => key) as never };
  return { disk, call, engine, proxyStore, jobs, deps, pushed };
}

const settle = async () => {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
};

const names = (call: ReturnType<typeof setup>["call"]) => call.mock.calls.map((c) => c[0]);

describe("the proxy badge", () => {
  it("shows nothing when there is no proxy, and each state of one", () => {
    expect(proxyBadge(undefined, {})).toBeNull();
    expect(proxyBadge(info(1, "none", { wanted: false }), {})).toBeNull();
    expect(proxyBadge(info(1, "none"), {})).toBeNull();
    expect(proxyBadge(info(1, "ready"), {})).toEqual({
      kind: "ready",
      percent: null,
      detail: null,
    });
    expect(proxyBadge(info(1, "queued"), {})?.kind).toBe("queued");
    expect(proxyBadge(info(1, "generating", { progress: 0.256 }), {})).toEqual({
      kind: "generating",
      percent: 25,
      detail: null,
    });
    expect(proxyBadge(info(1, "evicted"), {})?.kind).toBe("evicted");
    expect(proxyBadge(info(1, "incomplete"), {})?.kind).toBe("incomplete");
    expect(proxyBadge(info(1, "failed", { error: "disk full" }), {})).toEqual({
      kind: "failed",
      percent: null,
      detail: "disk full",
    });
  });

  it("follows the proxy job between two answers of the engine", () => {
    const queued = info(1, "queued", { job_id: jobId(1) });
    expect(proxyBadge(queued, { [jobId(1)]: job(1, "queued") })?.kind).toBe("queued");
    expect(proxyBadge(queued, { [jobId(1)]: job(1, "running", 0.5) })).toEqual({
      kind: "generating",
      percent: 50,
      detail: null,
    });
    // The job ended and the engine has not answered yet: what the job says.
    expect(proxyBadge(queued, { [jobId(1)]: job(1, "succeeded", 1) })?.kind).toBe("ready");
    expect(proxyBadge(queued, { [jobId(1)]: job(1, "failed") })).toEqual({
      kind: "failed",
      percent: null,
      detail: "disk full",
    });
    expect(proxyBadge(queued, { [jobId(1)]: job(1, "cancelled") })).toBeNull();
    // 100 % is "ready": a running job never shows it.
    expect(proxyBadge(queued, { [jobId(1)]: job(1, "running", 0.999) })?.percent).toBe(99);
  });
});

describe("proxy commands", () => {
  it("reads every proxy of the project", async () => {
    const s = setup([info(1, "ready"), info(2, "evicted")]);
    expect(await refreshProxies(s.deps)).toBe(true);
    expect(s.call).toHaveBeenCalledWith("media.proxy_status", { media: null });
    expect([...s.proxyStore.getState().byMedia.keys()]).toEqual([med(1), med(2)]);
  });

  it("regenerates: removes the ready ones, then makes every one again", async () => {
    const s = setup([info(1, "ready"), info(2, "evicted"), info(3, "generating")]);
    await refreshProxies(s.deps);
    s.call.mockClear();
    expect(await regenerateProxies(s.deps, [med(1), med(2), med(3), med(7)])).toBe(true);
    expect(s.call.mock.calls.slice(0, 2)).toEqual([
      ["media.delete_proxy", { media: [med(1)] }],
      ["media.generate_proxy", { media: [med(1), med(2), med(3)] }],
    ]);
    expect(s.proxyStore.getState().byMedia.get(med(1))?.status).toBe("queued");
    expect(s.proxyStore.getState().byMedia.get(med(2))?.status).toBe("queued");
    expect(s.pushed).toEqual([]);
  });

  it("deletes the proxies of the assets that have one", async () => {
    const s = setup([info(1, "ready"), info(2, "none", { wanted: false })]);
    await refreshProxies(s.deps);
    s.call.mockClear();
    expect(await deleteProxies(s.deps, [med(1), med(2)])).toBe(true);
    expect(s.call.mock.calls[0]).toEqual(["media.delete_proxy", { media: [med(1)] }]);
    expect(s.proxyStore.getState().byMedia.get(med(1))?.status).toBe("evicted");
  });

  it("says why when the engine refuses", async () => {
    const s = setup([info(1, "evicted")]);
    await refreshProxies(s.deps);
    s.disk.fail = true;
    expect(await regenerateProxies(s.deps, [med(1)])).toBe(false);
    expect(s.pushed).toEqual([
      expect.objectContaining({
        title: "mediaBin.proxy.failed.generate",
        message: "this engine makes no proxies",
      }),
    ]);
  });
});

describe("proxy wiring", () => {
  const project = (ids: number[]): Project =>
    ({
      media: Object.fromEntries(
        ids.map((n) => [med(n), { id: med(n), content_hash: `h${n}`, path_abs: `/m/${n}.mp4` }]),
      ),
    }) as unknown as Project;

  it("asks when it starts, when the media change, and when a proxy job starts or ends", async () => {
    const s = setup([info(1, "queued", { job_id: jobId(1) })]);
    const doc = createStore<{ project: Project | null }>()(() => ({ project: project([1]) }));
    const session = createStore<{ path: string | null; opening: unknown }>()(() => ({
      path: "/p/a.cutweave",
      opening: null,
    }));
    const stop = startProxyWiring({
      engine: s.engine,
      doc,
      session,
      jobs: s.jobs,
      proxies: s.proxyStore,
    });
    await settle();
    expect(names(s.call)).toEqual(["media.proxy_status"]);

    // An analysis job is not a proxy's business; progress of a running proxy job is not news.
    s.jobs.getState().applyProgress({ ...job(5, "running"), kind: "analysis", label: "index" });
    await settle();
    expect(s.call).toHaveBeenCalledTimes(1);
    s.jobs.getState().applyProgress(job(1, "running", 0.1));
    await settle();
    expect(s.call).toHaveBeenCalledTimes(2);
    s.jobs.getState().applyProgress(job(1, "running", 0.6));
    await settle();
    expect(s.call).toHaveBeenCalledTimes(2);
    s.disk.proxies = [info(1, "ready")];
    s.jobs.getState().applyProgress(job(1, "succeeded", 1));
    await settle();
    expect(s.call).toHaveBeenCalledTimes(3);
    expect(s.proxyStore.getState().byMedia.get(med(1))?.status).toBe("ready");

    // Another asset: asked again. An edit that changes no media: not asked.
    doc.setState({ project: project([1, 2]) });
    await settle();
    expect(s.call).toHaveBeenCalledTimes(4);
    doc.setState({ project: project([1, 2]) });
    await settle();
    expect(s.call).toHaveBeenCalledTimes(4);

    // Another project: the old answer goes at once.
    session.setState({ path: "/p/b.cutweave" });
    expect(s.proxyStore.getState().byMedia.size).toBe(0);
    await settle();
    expect(s.call).toHaveBeenCalledTimes(5);

    stop();
    s.jobs.getState().applyProgress(job(2, "queued"));
    await settle();
    expect(s.call).toHaveBeenCalledTimes(5);
  });
});
