// SPDX-License-Identifier: GPL-3.0-or-later
// Proxies in the bin (WP-2F.03b): where each video's proxy is, for the badge on its card and the
// card menu's "Regenerate proxies" and "Delete proxies". A proxy is cache state of this computer,
// never in the document, so the editor asks the engine (`media.proxy_status`) when a project
// opens, when the project's media change, when a proxy job starts or ends, and when the window
// gets the focus back (a cache limit or a clear may have removed some). Between two answers the
// badge follows the proxy job's `job.progress` events (`jobStore`).
//
// Both actions are catalog commands (`media.generate_proxy`, `media.delete_proxy`), so MCP and
// the CLI have the same calls. Neither changes the project: there is nothing to undo.
import {
  type JobId,
  type JobInfo,
  type MediaId,
  type Project,
  type ProxyInfo,
  RpcError,
} from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import { isActive, type JobState, jobStore } from "../state/jobStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { Translate } from "../timeline/errors.ts";

export interface ProxyState {
  /** The proxy of each asset that can have one, by asset id. */
  readonly byMedia: ReadonlyMap<string, ProxyInfo>;
  set(list: readonly ProxyInfo[]): void;
}

const NONE: ReadonlyMap<string, ProxyInfo> = new Map();

export function createProxyStore(): StoreApi<ProxyState> {
  return createStore<ProxyState>()((set) => ({
    byMedia: NONE,
    set: (list) =>
      set((s) => {
        if (list.length === 0 && s.byMedia.size === 0) return s;
        return { byMedia: new Map(list.map((p) => [p.media_id, p])) };
      }),
  }));
}

export const proxyStore = createProxyStore();

export type ProxyBadgeKind =
  | "ready"
  | "generating"
  | "queued"
  | "failed"
  | "evicted"
  | "incomplete";

/** What a card says about its proxy. */
export interface ProxyBadge {
  readonly kind: ProxyBadgeKind;
  /** While generating: how far it is, 0 to 99. */
  readonly percent: number | null;
  /** Why it failed. */
  readonly detail: string | null;
}

const badge = (kind: ProxyBadgeKind, detail: string | null = null): ProxyBadge => ({
  kind,
  percent: null,
  detail,
});

const generating = (fraction: number | null): ProxyBadge => ({
  kind: "generating",
  // 100 % is "ready": a proxy still being written never claims it.
  percent: Math.max(0, Math.min(99, Math.floor((fraction ?? 0) * 100))),
  detail: null,
});

/**
 * The badge of an asset: `null` when it has no proxy and none is being made. The engine's last
 * answer (`info`) names the job making the proxy; that job's newest state wins over the answer.
 */
export function proxyBadge(
  info: ProxyInfo | undefined,
  jobs: Readonly<Record<JobId, JobInfo>>,
): ProxyBadge | null {
  if (info === undefined) return null;
  const job = info.job_id === null ? undefined : jobs[info.job_id];
  if (job !== undefined) {
    if (isActive(job)) {
      return job.state === "running" ? generating(job.progress.fraction) : badge("queued");
    }
    // It ended and the engine is being asked again: say what the job said meanwhile.
    if (job.state === "succeeded") return badge("ready");
    if (job.state === "failed") return badge("failed", job.error);
    return null;
  }
  switch (info.status) {
    case "none":
      return null;
    case "generating":
      return generating(info.progress);
    case "failed":
      return badge("failed", info.error);
    default:
      return badge(info.status);
  }
}

/** The badge of one asset's card. */
export function useProxyBadge(id: string): ProxyBadge | null {
  const info = useStore(proxyStore, (s) => s.byMedia.get(id));
  const jobId = info?.job_id ?? null;
  const job = useStore(jobStore, (s) => (jobId === null ? undefined : s.jobs[jobId]));
  return proxyBadge(info, job === undefined ? {} : { [job.id]: job });
}

/** Whether the asset can have a proxy on this engine (a video with known content). */
export function canHaveProxy(info: ProxyInfo | undefined): boolean {
  return info !== undefined && info.profile !== null;
}

/** Whether the asset has a proxy, whole or in parts, or one is being made. */
export function hasProxy(info: ProxyInfo | undefined): boolean {
  return (
    info !== undefined &&
    info.profile !== null &&
    ["ready", "generating", "queued", "incomplete"].includes(info.status)
  );
}

export interface ProxyDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  readonly proxies?: StoreApi<ProxyState>;
}

const messageOf = (error: unknown): string =>
  error instanceof RpcError ? error.message : String(error);

/**
 * Asks the engine where the project's proxies are and stores the answer. `false` when there is
 * no engine or it could not say (the last answer stays).
 */
export async function refreshProxies(
  deps: Pick<ProxyDeps, "engine" | "proxies">,
): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return false;
  try {
    const out = await rpc.call("media.proxy_status", { media: null });
    (deps.proxies ?? proxyStore).getState().set(out.proxies);
    return true;
  } catch {
    return false;
  }
}

/**
 * Makes the proxies of `media` again: a ready one is removed first, one being made is left to
 * finish. Assets that cannot have a proxy (images, sound) are skipped.
 */
export async function regenerateProxies(
  deps: ProxyDeps,
  media: readonly MediaId[],
): Promise<boolean> {
  const store = deps.proxies ?? proxyStore;
  const rpc = deps.engine.getState().rpc;
  const known = store.getState().byMedia;
  const able = media.filter((id) => canHaveProxy(known.get(id)));
  if (rpc === null || able.length === 0) return false;
  try {
    const ready = able.filter((id) => known.get(id)?.status === "ready");
    if (ready.length > 0) await rpc.call("media.delete_proxy", { media: ready });
    await rpc.call("media.generate_proxy", { media: able });
    return true;
  } catch (error) {
    deps.toasts.getState().push({
      tone: "danger",
      title: deps.t("mediaBin.proxy.failed.generate"),
      message: messageOf(error),
    });
    return false;
  } finally {
    await refreshProxies(deps);
  }
}

/** Removes the proxies of `media` from the cache; the preview uses the originals again. */
export async function deleteProxies(deps: ProxyDeps, media: readonly MediaId[]): Promise<boolean> {
  const store = deps.proxies ?? proxyStore;
  const rpc = deps.engine.getState().rpc;
  const known = store.getState().byMedia;
  const having = media.filter((id) => hasProxy(known.get(id)));
  if (rpc === null || having.length === 0) return false;
  try {
    await rpc.call("media.delete_proxy", { media: having });
    return true;
  } catch (error) {
    deps.toasts.getState().push({
      tone: "danger",
      title: deps.t("mediaBin.proxy.failed.delete"),
      message: messageOf(error),
    });
    return false;
  } finally {
    await refreshProxies(deps);
  }
}

/** The project's media as one comparable text: which assets, their content and their files. */
function mediaOf(project: Project | null): string {
  if (project === null) return "";
  return Object.values(project.media)
    .map((m) => `${m.id}\u0000${m.content_hash ?? ""}\u0000${m.path_abs ?? ""}`)
    .sort()
    .join("\u0001");
}

/** The proxy jobs as one comparable text: which exist and which have started or ended. */
function proxyJobsOf(state: JobState): string {
  return Object.values(state.jobs)
    .filter((job) => job.kind === "proxy")
    .map((job) => `${job.id}:${job.state}`)
    .sort()
    .join(",");
}

export interface ProxyWiringDeps {
  readonly engine: StoreApi<EngineStoreState>;
  /** The document mirror: a change of the project's media asks again. */
  readonly doc: StoreApi<{ readonly project: Project | null }>;
  /** The project session: a newly opened project asks again. */
  readonly session: StoreApi<{ readonly path: string | null; readonly opening: unknown }>;
  readonly jobs: StoreApi<JobState>;
  readonly proxies?: StoreApi<ProxyState>;
  /** The window whose focus asks again (the cache may have changed); absent: none. */
  readonly window?: Pick<Window, "addEventListener" | "removeEventListener">;
}

/** Keeps the proxy store current (see the top of the file). Returns the function that stops it. */
export function startProxyWiring(deps: ProxyWiringDeps): () => void {
  const store = deps.proxies ?? proxyStore;
  let stopped = false;
  let running = false;
  let again = false;

  const check = async (): Promise<void> => {
    if (stopped) return;
    if (running) {
      again = true;
      return;
    }
    running = true;
    try {
      await refreshProxies({ engine: deps.engine, proxies: store });
    } finally {
      running = false;
      if (again) {
        again = false;
        void check();
      }
    }
  };

  /** Another project (or another engine): the old answer is about other files. */
  const opened = (): void => {
    store.getState().set([]);
    void check();
  };

  let media = mediaOf(deps.doc.getState().project);
  const offDoc = deps.doc.subscribe((state) => {
    const next = mediaOf(state.project);
    if (next === media) return;
    media = next;
    void check();
  });
  let jobs = proxyJobsOf(deps.jobs.getState());
  const offJobs = deps.jobs.subscribe((state) => {
    const next = proxyJobsOf(state);
    if (next === jobs) return;
    jobs = next;
    void check();
  });
  let project = deps.session.getState().path;
  let opening = deps.session.getState().opening !== null;
  const offSession = deps.session.subscribe((state) => {
    const nowOpening = state.opening !== null;
    const finished = opening && !nowOpening;
    opening = nowOpening;
    if (state.path === project && !finished) return;
    project = state.path;
    if (!nowOpening) opened();
  });
  let live = deps.engine.getState().rpc !== null;
  const offEngine = deps.engine.subscribe((state) => {
    const now = state.rpc !== null;
    if (now && !live) opened();
    live = now;
  });
  const onFocus = (): void => void check();
  deps.window?.addEventListener("focus", onFocus);
  if (live) void check();

  return () => {
    stopped = true;
    offDoc();
    offJobs();
    offSession();
    offEngine();
    deps.window?.removeEventListener("focus", onFocus);
  };
}
