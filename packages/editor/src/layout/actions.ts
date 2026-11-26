// SPDX-License-Identifier: GPL-3.0-or-later
// What the layout surfaces do (WP-2C.08): the Inspector's Layout row, the timeline's Layout menu
// and the bin's "Place side by side". Every placement is the engine's `item.apply_layout`, so the
// result is plain transform and crop values and an agent has the same call. "Place" is a batch:
// the inserts and the layout are one undo step.
import {
  type ItemId,
  type LayoutFill,
  type MediaId,
  RpcError,
  type TxId,
} from "@cutweave/protocol";
import { snapToFrame } from "@cutweave/timeline";
import type { StoreApi } from "zustand";
import type { EngineStoreState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { insertArgsFor } from "../menus/surfaces/bin.ts";
import { activeSequence, clipFacts } from "../menus/surfaces/facts.ts";
import { docStore } from "../state/docStore.ts";
import { type ToastStore, toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { findLayout, layoutsFor, SIDE_BY_SIDE } from "./table.ts";

export interface LayoutDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
}

/** How many clips a layout takes at most; the table has nothing for more. */
export const MAX_LAYOUT_CLIPS = 4;

/** The layout applied last to a set of clips, so "Swap" can reverse its order. */
interface Applied {
  readonly layout: string;
  readonly fill: LayoutFill;
  readonly order: readonly string[];
}

const applied = new Map<string, Applied>();
const keyOf = (ids: readonly string[]): string => [...ids].sort().join(",");

/** The layout last applied to exactly these clips by this editor, if any. */
export function lastApplied(ids: readonly string[]): Applied | undefined {
  return applied.get(keyOf(ids));
}

/** Forgets every remembered layout (tests; a new project starts with none). */
export function forgetAppliedLayouts(): void {
  applied.clear();
}

/** The video clips of `ids` (clips on video tracks) of the active sequence, in the given order. */
export function visualClipIds(ids: readonly string[]): string[] {
  return clipFacts(ids)
    .filter((f) => f.track.kind === "video")
    .map((f) => f.clip.id);
}

/** Whether 2 to 4 video clips are among `ids`, so a layout applies. */
export function canLayout(ids: readonly string[]): boolean {
  const n = visualClipIds(ids).length;
  return n >= 2 && n <= MAX_LAYOUT_CLIPS && layoutsFor(n).length > 0;
}

const messageOf = (error: unknown): string =>
  error instanceof RpcError ? error.message : String(error);

function fail(deps: LayoutDeps, error: unknown): void {
  deps.toasts.getState().push({
    tone: "danger",
    title: i18n.t("layout.failed"),
    message: messageOf(error),
  });
}

/** The playhead on the frame grid of the active sequence. */
function playhead(): number {
  const rate = activeSequence()?.sequence.format.frame_rate;
  const at = viewStore.getState().playhead;
  return rate === undefined ? at : snapToFrame(at, { num: rate.num, den: rate.den });
}

const defaultDeps = (engine: StoreApi<EngineStoreState>): LayoutDeps => ({
  engine,
  toasts: toastStore,
});

/**
 * `item.apply_layout` for the video clips among `ids`. `order` lists the clips for the cells;
 * without it the engine puts the top track first. Resolves to success.
 */
export async function applyLayout(
  engine: StoreApi<EngineStoreState>,
  ids: readonly string[],
  layout: string,
  options: { readonly fill?: LayoutFill; readonly order?: readonly string[] } = {},
): Promise<boolean> {
  const deps = defaultDeps(engine);
  const rpc = engine.getState().rpc;
  const items = visualClipIds(ids);
  if (rpc === null || items.length === 0) return false;
  const fill = options.fill ?? "crop";
  try {
    const out = await rpc.call("item.apply_layout", {
      items: items as ItemId[],
      layout,
      fill,
      ...(options.order === undefined ? {} : { order: options.order as ItemId[] }),
      at: playhead(),
    });
    applied.set(keyOf(items), {
      layout,
      fill,
      order: out.cells.map((c) => c.item),
    });
    return true;
  } catch (error) {
    fail(deps, error);
    return false;
  }
}

/** Reverses the cell order of the layout last applied to two clips. Resolves to success. */
export async function swapLayout(
  engine: StoreApi<EngineStoreState>,
  ids: readonly string[],
): Promise<boolean> {
  const items = visualClipIds(ids);
  const last = lastApplied(items);
  if (last === undefined) return false;
  return applyLayout(engine, items, last.layout, {
    fill: last.fill,
    order: [...last.order].reverse(),
  });
}

/** Whether Swap has a layout of these two clips to reverse. */
export function canSwap(ids: readonly string[]): boolean {
  const items = visualClipIds(ids);
  return items.length === 2 && lastApplied(items) !== undefined;
}

/** The asset kinds a layout takes: pictures that fill a cell. */
export function placeable(media: readonly string[]): boolean {
  const project = docStore.getState().project;
  return (
    media.length >= 2 &&
    media.length <= MAX_LAYOUT_CLIPS &&
    media.every((id) => {
      const kind = project?.media[id as MediaId]?.kind;
      return kind === "video" || kind === "image";
    })
  );
}

/**
 * Inserts the assets at the playhead, each on a track of its own, and applies the layout, as one
 * undo step. `layout` defaults to side by side for two, columns for three and a grid for four.
 * The first asset takes the first cell. Resolves to the new clips, or `[]` after a toast.
 */
export async function placeAssets(
  engine: StoreApi<EngineStoreState>,
  media: readonly string[],
  layout: string | undefined,
): Promise<string[]> {
  const deps = defaultDeps(engine);
  const rpc = engine.getState().rpc;
  const chosen = layout ?? SIDE_BY_SIDE[media.length];
  if (rpc === null || chosen === undefined || findLayout(chosen) === undefined) return [];
  let tx: TxId | null = null;
  try {
    const opened = await rpc.call("edit.begin_tx", {
      kind: "batch",
      label: i18n.t("history.placeLayout"),
    });
    tx = opened.tx;
    const clips: ItemId[] = [];
    for (const id of media) {
      const args = insertArgsFor(id, "playhead");
      if (args === undefined) throw new Error(i18n.t("contextMenu.reason.noTrack"));
      const out = await rpc.call("timeline.insert", { ...args, media: id as MediaId } as never, {
        tx,
      });
      const clip = out.clips[0];
      if (clip === undefined) throw new Error(i18n.t("layout.failed"));
      clips.push(clip);
    }
    const out = await rpc.call(
      "item.apply_layout",
      { items: clips, layout: chosen, fill: "crop", order: clips, at: playhead() },
      { tx },
    );
    await rpc.call("edit.commit_tx", { tx });
    applied.set(keyOf(clips), {
      layout: chosen,
      fill: "crop",
      order: out.cells.map((c) => c.item),
    });
    viewStore.getState().setSelection(clips.map((id) => ({ kind: "item", id })));
    return clips;
  } catch (error) {
    if (tx !== null) await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
    fail(deps, error);
    return [];
  }
}
