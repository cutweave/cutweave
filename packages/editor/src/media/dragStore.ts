// SPDX-License-Identifier: GPL-3.0-or-later
// The drag session of a media card (WP-1M.03). Chromium hides `dataTransfer` data during
// `dragover` (only the types are readable), so what is being dragged lives here: the card
// begins the session on `dragstart`, the timeline reads it on every `dragover` and `drop`, and
// whoever sees the drag end clears it. `dataTransfer` still carries the media ID under
// `MEDIA_DRAG_TYPE`, so a drop anywhere else can tell it is ours.
import type { DroppedMedia } from "@cutweave/timeline";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { DragPayload } from "./folders.ts";

/** The `dataTransfer` type a card sets (the value is the media ID). */
export const MEDIA_DRAG_TYPE = "application/x-cutweave-media";

/**
 * What is dragged to the timeline: a bin card, or the Source view's marked range (WP-2A.14).
 * `duration` is then the range's length (the drop's ghost shows it).
 */
export interface DraggedMedia extends DroppedMedia {
  /** The part of the media to place (flicks, the media's own time): the Source view's marks. */
  readonly range?: { readonly sourceIn: number; readonly duration: number };
  /** `false`: the picture without its sound (the Source view's picture icon). */
  readonly link?: false;
}

export interface DragState {
  /** The card being dragged, or `null` when no drag from the bin is under way. */
  readonly session: DraggedMedia | null;
  begin(media: DraggedMedia): void;
  end(): void;
}

export function createDragStore() {
  return createStore<DragState>()((set) => ({
    session: null,
    begin: (media) => set({ session: media }),
    end: () => set({ session: null }),
  }));
}

export const dragStore = createDragStore();

export function useDragStore<T>(selector: (state: DragState) => T): T {
  return useStore(dragStore, selector);
}

/** The `dataTransfer` type of tiles dragged inside the bin (WP-2F.08); the value is a placeholder. */
export const TILE_DRAG_TYPE = "application/x-cutweave-bin-tiles";

export interface TileDragState {
  /** The assets and folders being dragged inside the bin, or `null`. */
  readonly payload: DragPayload | null;
  begin(payload: DragPayload): void;
  end(): void;
}

/**
 * Tiles dragged onto folders and breadcrumb segments (WP-2F.08). Like the media session above,
 * it lives here because Chromium hides `dataTransfer` data during `dragover`.
 */
export const tileDragStore = createStore<TileDragState>()((set) => ({
  payload: null,
  begin: (payload) => set({ payload }),
  end: () => set({ payload: null }),
}));
