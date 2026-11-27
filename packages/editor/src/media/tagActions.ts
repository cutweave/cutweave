// SPDX-License-Identifier: GPL-3.0-or-later
// What the bin does to tags, ratings and notes (WP-2F.06b, ADR-0036), each as one catalog command
// so MCP and the CLI have the same call and each is one undo step: `media.add_tags`,
// `media.remove_tags`, `media.set_rating`, `media.set_note`. A command takes at most 256 assets;
// a larger selection goes in one transaction (one undo step still). The engine is the authority
// on what a tag or a note may be: its message is shown as it is.
import { type MediaId, RpcError, type TxId } from "@cutweave/protocol";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import type { BinDeps } from "./binActions.ts";
import { MAX_TAG_MEDIA } from "./tags.ts";

const messageOf = (error: unknown): string =>
  error instanceof RpcError ? error.message : String(error);

type Rpc = NonNullable<EngineStoreState["rpc"]>;
/** One command for (a part of) the assets; `options` carries the transaction when there is one. */
type Send = (rpc: Rpc, media: MediaId[], options?: { tx: TxId }) => Promise<unknown>;

/** Sends `send` for `media`, 256 at a time inside one transaction when there are more. */
async function forMedia(
  deps: BinDeps,
  media: readonly MediaId[],
  label: TKey,
  failed: TKey,
  send: Send,
): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null || media.length === 0) return false;
  let tx: TxId | null = null;
  try {
    if (media.length <= MAX_TAG_MEDIA) {
      await send(rpc, [...media]);
      return true;
    }
    const opened = await rpc.call("edit.begin_tx", { kind: "batch", label: deps.t(label) });
    tx = opened.tx;
    for (let i = 0; i < media.length; i += MAX_TAG_MEDIA) {
      await send(rpc, media.slice(i, i + MAX_TAG_MEDIA), { tx });
    }
    await rpc.call("edit.commit_tx", { tx });
    return true;
  } catch (error) {
    if (tx !== null) await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
    deps.toasts.getState().push({ tone: "danger", title: deps.t(failed), message: messageOf(error) });
    return false;
  }
}

/** Adds `tags` to every asset of `media`: one undo step. Resolves to success. */
export function addTags(
  deps: BinDeps,
  media: readonly MediaId[],
  tags: readonly string[],
): Promise<boolean> {
  if (tags.length === 0) return Promise.resolve(false);
  return forMedia(deps, media, "history.tagAdd", "mediaBin.tags.addFailed", (rpc, ids, options) =>
    rpc.call("media.add_tags", { media: ids, tags: [...tags] }, options),
  );
}

/** Takes `tags` off every asset of `media`: one undo step. */
export function removeTags(
  deps: BinDeps,
  media: readonly MediaId[],
  tags: readonly string[],
): Promise<boolean> {
  if (tags.length === 0) return Promise.resolve(false);
  return forMedia(
    deps,
    media,
    "history.tagRemove",
    "mediaBin.tags.removeFailed",
    (rpc, ids, options) => rpc.call("media.remove_tags", { media: ids, tags: [...tags] }, options),
  );
}

/** Rates every asset of `media` (`null`: unrated): one undo step. */
export function setRating(
  deps: BinDeps,
  media: readonly MediaId[],
  rating: number | null,
): Promise<boolean> {
  return forMedia(deps, media, "history.rating", "mediaBin.rating.failed", (rpc, ids, options) =>
    rpc.call("media.set_rating", { media: ids, rating }, options),
  );
}

/** Sets the note of one asset (`null`: none): one undo step. */
export async function setNote(deps: BinDeps, media: MediaId, note: string | null): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return false;
  try {
    await rpc.call("media.set_note", { media, note });
    return true;
  } catch (error) {
    deps.toasts
      .getState()
      .push({ tone: "danger", title: deps.t("mediaBin.note.failed"), message: messageOf(error) });
    return false;
  }
}
