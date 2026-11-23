// SPDX-License-Identifier: GPL-3.0-or-later
// The drag session of a media card (WP-1M.03). Chromium hides `dataTransfer` data during
// `dragover` (only the types are readable), so what is being dragged lives here: the card
// begins the session on `dragstart`, the timeline reads it on every `dragover` and `drop`, and
// whoever sees the drag end clears it. `dataTransfer` still carries the media ID under
// `MEDIA_DRAG_TYPE`, so a drop anywhere else can tell it is ours.
import type { MediaDrag } from "@cutweave/timeline";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

/** The `dataTransfer` type a card sets (the value is the media ID). */
export const MEDIA_DRAG_TYPE = "application/x-cutweave-media";

export interface DragState {
  /** The card being dragged, or `null` when no drag from the bin is under way. */
  readonly session: MediaDrag | null;
  begin(media: MediaDrag): void;
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
