// SPDX-License-Identifier: GPL-3.0-or-later
// Media health (WP-2F.03): which assets' files are missing on this computer, and the relink
// actions. Health is never in the document (it is a fact about this machine), so the editor asks
// the engine (`media.find_missing`) when a project opens, when the window gets the focus back,
// and when a media path changes (a relink, its undo, an agent's relink). The bin's badges, the
// "Relink…" menu rows and the relink dialog read `healthStore`.
// An image sequence with pictures gone is in the list too (`incomplete`, or `missing` when its
// first picture is gone), so it shows as offline and the dialog says how many are missing (#208).
//
// Relinking is `media.relink_folder` (search a folder: by content, then by name) or
// `media.relink` (one asset, one file): catalog commands, so MCP and the CLI have the same calls,
// and each is one undo step.
import { type MediaId, type MissingMedia, type Project, RpcError } from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { Translate } from "../timeline/errors.ts";

export interface RelinkSummary {
  /** How many assets were pointed at a file. */
  readonly relinked: number;
  /** How many assets are still missing afterwards. */
  readonly stillMissing: number;
  /** The folder holds more files than one search reads. */
  readonly truncated: boolean;
}

export interface HealthState {
  /** The assets whose file is missing, changed or (an image sequence) incomplete, by asset id. */
  readonly missing: ReadonlyMap<string, MissingMedia>;
  /** The relink dialog is open. */
  readonly dialogOpen: boolean;
  /** A relink is under way. */
  readonly busy: boolean;
  /** What the last relink of this dialog did; `null` before the first. */
  readonly last: RelinkSummary | null;
  setMissing(list: readonly MissingMedia[]): void;
  openDialog(): void;
  closeDialog(): void;
  setBusy(busy: boolean): void;
  setLast(last: RelinkSummary | null): void;
}

const NONE: ReadonlyMap<string, MissingMedia> = new Map();

export function createHealthStore(): StoreApi<HealthState> {
  return createStore<HealthState>()((set) => ({
    missing: NONE,
    dialogOpen: false,
    busy: false,
    last: null,
    setMissing: (list) =>
      set((s) => {
        if (list.length === 0 && s.missing.size === 0) return s;
        return { missing: new Map(list.map((m) => [m.media_id, m])) };
      }),
    openDialog: () => set({ dialogOpen: true, last: null }),
    closeDialog: () => set({ dialogOpen: false, busy: false }),
    setBusy: (busy) => set({ busy }),
    setLast: (last) => set({ last }),
  }));
}

export const healthStore = createHealthStore();

/** The missing assets, by name (the order the dialog lists them in). */
export function sortMissing(missing: ReadonlyMap<string, MissingMedia>): readonly MissingMedia[] {
  return [...missing.values()].sort(
    (a, b) => a.name.localeCompare(b.name) || a.media_id.localeCompare(b.media_id),
  );
}

/** Whether the asset's file is missing or changed. */
export function useIsOffline(id: string): boolean {
  return useStore(healthStore, (s) => s.missing.has(id));
}

export interface HealthDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  readonly health?: StoreApi<HealthState>;
}

const messageOf = (error: unknown): string =>
  error instanceof RpcError ? error.message : String(error);

/**
 * Asks the engine which media are missing and stores the answer. Resolves to how many there are;
 * `null` when there is no engine or it could not say (the last answer stays).
 */
export async function refreshHealth(
  deps: Pick<HealthDeps, "engine" | "health">,
): Promise<number | null> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return null;
  try {
    const out = await rpc.call("media.find_missing", { verify: null });
    (deps.health ?? healthStore).getState().setMissing(out.missing);
    return out.missing.length;
  } catch {
    return null;
  }
}

/**
 * Searches `folder` (and the folders inside it) for the missing media and relinks what it finds:
 * one `media.relink_folder`, one undo step. The summary stays in the dialog; a failure is a toast.
 */
export async function relinkFromFolder(
  deps: HealthDeps,
  folder: string,
): Promise<RelinkSummary | null> {
  const health = deps.health ?? healthStore;
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return null;
  health.getState().setBusy(true);
  try {
    const out = await rpc.call("media.relink_folder", { folder, recursive: null });
    await refreshHealth(deps);
    const summary: RelinkSummary = {
      relinked: out.relinked.length,
      stillMissing: health.getState().missing.size,
      truncated: out.truncated,
    };
    health.getState().setLast(summary);
    return summary;
  } catch (error) {
    deps.toasts.getState().push({
      tone: "danger",
      title: deps.t("relink.failed.folder"),
      message: messageOf(error),
    });
    return null;
  } finally {
    health.getState().setBusy(false);
  }
}

/** Points one asset at `path` (`media.relink`): the moved original, or a file to use instead. */
export async function relinkOne(deps: HealthDeps, media: MediaId, path: string): Promise<boolean> {
  const health = deps.health ?? healthStore;
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return false;
  health.getState().setBusy(true);
  try {
    await rpc.call("media.relink", { media, path });
    await refreshHealth(deps);
    health.getState().setLast({
      relinked: 1,
      stillMissing: health.getState().missing.size,
      truncated: false,
    });
    return true;
  } catch (error) {
    deps.toasts.getState().push({
      tone: "danger",
      title: deps.t("relink.failed.file"),
      message: messageOf(error),
    });
    return false;
  } finally {
    health.getState().setBusy(false);
  }
}

/** The media paths of a project, as one comparable text. */
function pathsOf(project: Project | null): string {
  if (project === null) return "";
  return Object.values(project.media)
    .map((m) => `${m.id}\u0000${m.path_abs ?? ""}`)
    .sort()
    .join("\u0001");
}

export interface HealthWiringDeps {
  readonly engine: StoreApi<EngineStoreState>;
  /** The document mirror: a change of a media path asks again. */
  readonly doc: StoreApi<{ readonly project: Project | null }>;
  /** The project session: a newly opened project asks again and shows the dialog if needed. */
  readonly session: StoreApi<{ readonly path: string | null; readonly opening: unknown }>;
  readonly health?: StoreApi<HealthState>;
  /** The window whose focus asks again (the files may have come back); absent: none. */
  readonly window?: Pick<Window, "addEventListener" | "removeEventListener">;
}

/**
 * Keeps the health store current (see the top of the file). Opening a project with missing media
 * opens the relink dialog once. Returns the function that stops it.
 */
export function startHealthWiring(deps: HealthWiringDeps): () => void {
  const health = deps.health ?? healthStore;
  let stopped = false;
  let running = false;
  let again = false;
  let announce = false;

  const check = async (): Promise<void> => {
    if (stopped) return;
    if (running) {
      again = true;
      return;
    }
    running = true;
    try {
      const count = await refreshHealth({ engine: deps.engine, health });
      if (announce && count !== null && !again) {
        announce = false;
        if (count > 0 && !stopped) health.getState().openDialog();
      }
    } finally {
      running = false;
      if (again) {
        again = false;
        void check();
      }
    }
  };

  /** The project the editor shows changed: its health is announced once it is known. */
  const opened = (): void => {
    health.getState().closeDialog();
    health.getState().setMissing([]);
    announce = true;
    void check();
  };

  let paths = pathsOf(deps.doc.getState().project);
  const offDoc = deps.doc.subscribe((state) => {
    const next = pathsOf(state.project);
    if (next === paths) return;
    paths = next;
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
  if (live) opened();

  return () => {
    stopped = true;
    offDoc();
    offSession();
    offEngine();
    deps.window?.removeEventListener("focus", onFocus);
  };
}
