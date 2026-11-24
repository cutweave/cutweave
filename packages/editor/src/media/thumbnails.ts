// SPDX-License-Identifier: GPL-3.0-or-later
// Where a media card gets its picture (WP-1M.03). The panel never builds a thumbnail URL itself:
// it asks the current `ThumbnailSource`, which says either "draw the design-system placeholder
// tile" or "load this image". Without an engine that serves media the source is
// `placeholderThumbnails`, so every card shows its tile.
//
// WP-1I.03 plugs in `HttpThumbnailSource` (httpThumbnails.ts) with `setThumbnailSource(source)`
// once the engine connection is up and the engine has media (mediaWiring.ts): `thumbnailFor`
// returns `{ kind: "image", src }` for `GET /media/{id}/thumb` with a signed URL (`POST
// /media/sign`); `src` never carries the UI token (api.ts: never in a URL).
// `request.indexed` changes when `media.indexed` arrives, so a source can answer "placeholder"
// before the frame index exists and the real URL afterwards (the panel re-asks on every change);
// `request.width` is the card's CSS width, for picking a size. `onError` falls the card back to
// the tile (a failed or missing thumbnail never shows a broken-image glyph).
import type { MediaId, MediaKind } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export interface ThumbnailRequest {
  readonly id: MediaId;
  readonly kind: MediaKind;
  /** The card's width in CSS pixels. */
  readonly width: number;
  /** How often the engine has indexed this asset (`media.indexed`); 0 before the first time. */
  readonly indexed: number;
}

export type Thumbnail =
  /** The design-system tile: an icon for the kind on `surface-3`. */
  | { readonly kind: "placeholder" }
  /** An image to load; the card falls back to the tile if it fails. */
  | { readonly kind: "image"; readonly src: string };

export interface ThumbnailSource {
  thumbnailFor(request: ThumbnailRequest): Thumbnail;
}

const PLACEHOLDER: Thumbnail = { kind: "placeholder" };

/** The source until WP-1I.03: every card draws the placeholder tile. */
export const placeholderThumbnails: ThumbnailSource = {
  thumbnailFor: () => PLACEHOLDER,
};

export interface ThumbnailSourceState {
  readonly source: ThumbnailSource;
}

export const thumbnailSourceStore = createStore<ThumbnailSourceState>(() => ({
  source: placeholderThumbnails,
}));

/** Installs the source the cards ask (WP-1I.03). Pass `placeholderThumbnails` to go back. */
export function setThumbnailSource(source: ThumbnailSource): void {
  thumbnailSourceStore.setState({ source });
}

export function useThumbnailSource(): ThumbnailSource {
  return useStore(thumbnailSourceStore, (s) => s.source);
}
