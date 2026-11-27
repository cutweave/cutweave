// SPDX-License-Identifier: GPL-3.0-or-later
// What the bin does to collections (WP-2F.06c, ADR-0036), each as one catalog command so MCP and
// the CLI have the same call and each is one undo step: `media.create_collection`,
// `media.rename_collection`, `media.delete_collection`, `media.add_to_collection`,
// `media.remove_from_collection`, `media.set_collection_filter`. A command takes up to 10,000
// assets, as many as a collection holds, so a selection never needs a second call. The engine is
// the authority on names and members: its message is shown as it is. Deleting a collection never
// removes media.
import { type CollectionId, type CollectionKind, type MediaId, RpcError } from "@cutweave/protocol";
import type { BinDeps } from "./binActions.ts";
import type { MediaFilter } from "./find.ts";

const messageOf = (error: unknown): string =>
  error instanceof RpcError ? error.message : String(error);

function fail(deps: BinDeps, title: string, error: unknown): void {
  deps.toasts.getState().push({ tone: "danger", title, message: messageOf(error) });
}

/** Creates a collection: manual with `media` (may be empty), or smart with `filter`. */
export async function createCollection(
  deps: BinDeps,
  name: string,
  kind: CollectionKind,
): Promise<CollectionId | null> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return null;
  try {
    const out = await rpc.call("media.create_collection", { name: name.trim(), kind });
    return out.collection_id;
  } catch (error) {
    fail(deps, deps.t("mediaBin.collections.createFailed"), error);
    return null;
  }
}

/** Renames a collection. A blank or unchanged name is not a command. Resolves to success. */
export async function renameCollection(
  deps: BinDeps,
  collection: CollectionId,
  from: string,
  name: string,
): Promise<boolean> {
  const next = name.trim();
  if (next === "" || next === from) return false;
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return false;
  try {
    await rpc.call("media.rename_collection", { collection, name: next });
    return true;
  } catch (error) {
    fail(deps, deps.t("mediaBin.collections.renameFailed", { name: from }), error);
    return false;
  }
}

/** Deletes a collection; its media stay in the project. */
export async function deleteCollection(
  deps: BinDeps,
  collection: CollectionId,
  name: string,
): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return false;
  try {
    await rpc.call("media.delete_collection", { collection });
    return true;
  } catch (error) {
    fail(deps, deps.t("mediaBin.collections.deleteFailed", { name }), error);
    return false;
  }
}

/**
 * Adds `media` to a manual collection: one undo step. Resolves to how many were new to it, or
 * `null` after a toast.
 */
export async function addToCollection(
  deps: BinDeps,
  collection: CollectionId,
  media: readonly MediaId[],
): Promise<number | null> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null || media.length === 0) return null;
  try {
    const out = await rpc.call("media.add_to_collection", { collection, media: [...media] });
    return out.added.length;
  } catch (error) {
    fail(deps, deps.t("mediaBin.collections.addFailed"), error);
    return null;
  }
}

/** Takes `media` off a manual collection: one undo step. The media stay in the project. */
export async function removeFromCollection(
  deps: BinDeps,
  collection: CollectionId,
  media: readonly MediaId[],
): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null || media.length === 0) return false;
  try {
    await rpc.call("media.remove_from_collection", { collection, media: [...media] });
    return true;
  } catch (error) {
    fail(deps, deps.t("mediaBin.collections.removeFailed"), error);
    return false;
  }
}

/** Replaces a smart collection's filter. Resolves to how many assets pass it, or `null`. */
export async function setCollectionFilter(
  deps: BinDeps,
  collection: CollectionId,
  filter: MediaFilter,
): Promise<number | null> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return null;
  try {
    const out = await rpc.call("media.set_collection_filter", { collection, filter });
    return out.members;
  } catch (error) {
    fail(deps, deps.t("mediaBin.collections.updateFailed"), error);
    return null;
  }
}
