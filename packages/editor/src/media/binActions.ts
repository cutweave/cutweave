// SPDX-License-Identifier: GPL-3.0-or-later
// What the bin does to folders and assets (WP-2F.08, ADR-0020), each as one catalog command so
// MCP and the CLI have the same call and each is one undo step: `media.create_folder`,
// `media.rename_folder`, `media.rename`, `media.move`, `media.delete_folder`. The engine is the
// authority on names and cycles (arch §5.1): its message is shown as it is. Moves always send
// `on_conflict: "rename"`, so a name clash never blocks a drop; a toast says what was renamed.
import {
  type FolderContents,
  type FolderId,
  type MediaId,
  type MediaMoved,
  RpcError,
  type TxId,
} from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { Translate } from "../timeline/errors.ts";

export interface BinDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
}

const messageOf = (error: unknown): string =>
  error instanceof RpcError ? error.message : String(error);

function fail(deps: BinDeps, title: string, error: unknown): void {
  deps.toasts.getState().push({ tone: "danger", title, message: messageOf(error) });
}

/**
 * Creates a folder in `parent` (`null`: the top level) named by the engine ("New folder", "New
 * folder 2", ...), or `name` when given. Resolves to its ID, or `null` after a toast.
 */
export async function createFolder(
  deps: BinDeps,
  parent: FolderId | null,
  name?: string,
): Promise<FolderId | null> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return null;
  try {
    const out = await rpc.call("media.create_folder", {
      parent,
      ...(name === undefined ? {} : { name }),
      on_conflict: "rename",
    });
    return out.folder_id;
  } catch (error) {
    fail(deps, deps.t("mediaBin.folder.createFailed"), error);
    return null;
  }
}

/** Renames a folder. A blank or unchanged name is not a command. Resolves to success. */
export async function renameFolder(
  deps: BinDeps,
  folder: FolderId,
  from: string,
  name: string,
): Promise<boolean> {
  const next = name.trim();
  if (next === "" || next === from) return false;
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return false;
  try {
    await rpc.call("media.rename_folder", { folder, name: next });
    return true;
  } catch (error) {
    fail(deps, deps.t("mediaBin.folder.renameFailed", { name: from }), error);
    return false;
  }
}

/** Gives an asset a display name (the file on disk keeps its name). Blank or unchanged: nothing. */
export async function renameMedia(
  deps: BinDeps,
  media: MediaId,
  from: string,
  name: string,
): Promise<boolean> {
  const next = name.trim();
  if (next === "" || next === from) return false;
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return false;
  try {
    await rpc.call("media.rename", { media, name: next });
    return true;
  } catch (error) {
    fail(deps, deps.t("mediaBin.folder.renameFailed", { name: from }), error);
    return false;
  }
}

/** The toast text for folders the engine renamed while moving. */
export function renamedNotice(moved: MediaMoved, t: Translate): string | null {
  const count = moved.renamed.length;
  if (count === 0) return null;
  return count === 1
    ? t("mediaBin.folder.renamedOne")
    : t("mediaBin.folder.renamedMany", { count: String(count) });
}

/**
 * Moves assets and folders into `to` (`null`: the top level) as one undo step. Resolves to the
 * engine's answer, or `null` after a toast.
 */
export async function moveTiles(
  deps: BinDeps,
  items: { readonly media: readonly MediaId[]; readonly folders: readonly FolderId[] },
  to: FolderId | null,
): Promise<MediaMoved | null> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null || (items.media.length === 0 && items.folders.length === 0)) return null;
  try {
    const moved = await rpc.call("media.move", {
      ...(items.media.length > 0 ? { media: [...items.media] } : {}),
      ...(items.folders.length > 0 ? { folders: [...items.folders] } : {}),
      to,
      on_conflict: "rename",
    });
    const notice = renamedNotice(moved, deps.t);
    if (notice !== null) deps.toasts.getState().push({ tone: "info", title: notice });
    return moved;
  } catch (error) {
    fail(deps, deps.t("mediaBin.folder.moveFailed"), error);
    return null;
  }
}

/** Deletes a folder: `remove` takes its unused media with it, `move_up` lifts everything. */
export async function deleteFolder(
  deps: BinDeps,
  folder: FolderId,
  name: string,
  contents: FolderContents,
): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return false;
  try {
    await rpc.call("media.delete_folder", { folder, contents });
    return true;
  } catch (error) {
    fail(deps, deps.t("mediaBin.folder.deleteFailed", { name }), error);
    return false;
  }
}

/**
 * Removes several assets from the project as one undo step (`media.remove` in a batch). The ones
 * clips use are refused with the engine's own message and the rest still go. Resolves to the IDs
 * removed.
 */
export async function removeManyMedia(
  deps: BinDeps,
  media: readonly { readonly id: MediaId; readonly name: string; readonly used: boolean }[],
): Promise<MediaId[]> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return [];
  const free = media.filter((m) => !m.used);
  const kept = media.filter((m) => m.used);
  const removed: MediaId[] = [];
  if (free.length > 0) {
    let tx: TxId | null = null;
    try {
      const opened = await rpc.call("edit.begin_tx", {
        kind: "batch",
        label: deps.t("mediaBin.remove.button"),
      });
      tx = opened.tx;
      for (const m of free) {
        await rpc.call("media.remove", { id: m.id }, { tx });
        removed.push(m.id);
      }
      await rpc.call("edit.commit_tx", { tx });
    } catch (error) {
      if (tx !== null) await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
      fail(deps, deps.t("mediaBin.remove.failedTitle", { name: free[0]?.name ?? "" }), error);
      return [];
    }
  }
  const first = kept[0];
  if (first !== undefined) {
    // The engine words the refusal ("N clips use it; delete those clips first").
    try {
      await rpc.call("media.remove", { id: first.id });
      removed.push(first.id);
    } catch (error) {
      deps.toasts.getState().push({
        tone: "danger",
        title:
          kept.length === 1
            ? deps.t("mediaBin.remove.failedTitle", { name: first.name })
            : deps.t("mediaBin.remove.keptMany", { count: String(kept.length) }),
        message: messageOf(error),
      });
    }
  }
  return removed;
}
