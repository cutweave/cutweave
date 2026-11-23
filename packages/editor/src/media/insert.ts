// SPDX-License-Identifier: GPL-3.0-or-later
// Putting a bin asset on the timeline (WP-1M.03): one `timeline.insert` with `overlap: "new_track"`
// so a drop or an Enter never fails on an occupied spot (arch §5.1: the engine decides; the
// result comes back as `doc.patched`). Shared by the timeline drop (`TimelineController`) and the
// media panel's keyboard path.
import type { ItemId, MediaId, RpcClient, SelectionRef, TrackId } from "@cutweave/protocol";
import { snapToFrame } from "@cutweave/timeline";
import type { StoreApi } from "zustand";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { DocStore } from "../state/docStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { ViewStore } from "../state/viewStore.ts";
import { describeRefusal, FAILED_TITLE, type Translate } from "../timeline/errors.ts";
import { toRpcError } from "../timeline/gesture.ts";

export interface InsertDeps {
  readonly doc: StoreApi<DocStore>;
  readonly view: StoreApi<ViewStore>;
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
}

/**
 * Inserts `media` on `track` at `at` flicks (the engine snaps to its frame grid again). Selects
 * the new clip. A refusal shows a toast and resolves to `false`; nothing throws.
 */
export async function insertMedia(
  deps: InsertDeps,
  media: string,
  track: string,
  at: number,
): Promise<boolean> {
  const rpc: RpcClient | null = deps.engine.getState().rpc;
  const sequence = selectActiveSequenceId(deps.doc.getState().project);
  if (rpc === null || sequence === null) return false;
  try {
    const out = await rpc.call("timeline.insert", {
      sequence,
      track: track as TrackId,
      media: media as MediaId,
      at,
      overlap: "new_track",
    });
    const first = out.clips[0];
    if (first !== undefined) {
      deps.view.getState().setSelection([{ kind: "item", id: first as ItemId } as SelectionRef]);
    }
    return true;
  } catch (error) {
    deps.toasts.getState().push({
      tone: "warning",
      title: deps.t(FAILED_TITLE.insert),
      message: describeRefusal(toRpcError(error), deps.t),
    });
    return false;
  }
}

/**
 * The track Enter inserts on: the selected track when it takes this kind of media, else the
 * first unlocked track that does (`null` when the sequence has none).
 */
export function pickTrack(
  deps: Pick<InsertDeps, "doc" | "view">,
  kind: "video" | "audio" | "image",
): TrackId | null {
  const project = deps.doc.getState().project;
  const sequenceId = selectActiveSequenceId(project);
  const sequence = sequenceId === null ? undefined : project?.sequences[sequenceId];
  if (sequence === undefined) return null;
  const want = kind === "audio" ? "audio" : "video";
  const fits = (id: TrackId): boolean => sequence.tracks[id]?.kind === want;
  const selected = deps.view
    .getState()
    .selection.find((r) => r.kind === "track" && fits(r.id as TrackId));
  if (selected !== undefined) return selected.id as TrackId;
  const first = sequence.track_order.find((id) => fits(id) && sequence.tracks[id]?.locked !== true);
  return first ?? null;
}

/** Inserts at the playhead on the selected (or first fitting) track. */
export async function insertAtPlayhead(
  deps: InsertDeps,
  media: { readonly id: string; readonly kind: "video" | "audio" | "image" },
): Promise<boolean> {
  const track = pickTrack(deps, media.kind);
  const project = deps.doc.getState().project;
  const sequenceId = selectActiveSequenceId(project);
  const sequence = sequenceId === null ? undefined : project?.sequences[sequenceId];
  if (track === null || sequence === undefined) return false;
  const fps = { num: sequence.format.frame_rate.num, den: sequence.format.frame_rate.den };
  return insertMedia(deps, media.id, track, snapToFrame(deps.view.getState().playhead, fps));
}
