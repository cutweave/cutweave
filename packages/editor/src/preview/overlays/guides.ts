// SPDX-License-Identifier: GPL-3.0-or-later
// The preview's guides (WP-2H.06): which are on, and their geometry. Session state, like the
// safe-area toggle; none of it is part of the project.
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import safeAreaData from "./safe-areas.json";

export interface PlatformSafeArea {
  readonly id: string;
  readonly name: string;
  /** Fractions of the frame kept clear at each edge. */
  readonly insets: {
    readonly top: number;
    readonly bottom: number;
    readonly left: number;
    readonly right: number;
  };
}

/** The safe-area data pack: the generic action and title insets, then the platforms'. */
export const SAFE_AREA_PACK = safeAreaData as {
  readonly version: number;
  readonly generic: readonly { readonly id: "action" | "title"; readonly inset: number }[];
  readonly platforms: readonly PlatformSafeArea[];
};

export const platformSafeArea = (id: string | null): PlatformSafeArea | undefined =>
  SAFE_AREA_PACK.platforms.find((p) => p.id === id);

/** The aspect-ratio frame guides (width : height). */
export const ASPECT_GUIDES = [
  { id: "16:9", ratio: 16 / 9 },
  { id: "9:16", ratio: 9 / 16 },
  { id: "1:1", ratio: 1 },
  { id: "4:5", ratio: 4 / 5 },
  { id: "4:3", ratio: 4 / 3 },
  { id: "2.39:1", ratio: 2.39 },
] as const;
export type AspectGuideId = (typeof ASPECT_GUIDES)[number]["id"];

/** Custom grid divisions the menu offers (columns and rows). */
export const GRID_DIVISIONS = [4, 8, 16] as const;
export type GridDivisions = (typeof GRID_DIVISIONS)[number];

export interface GuideState {
  /** A platform's UI-safe zone, drawn with the safe areas (`null`: the generic ones only). */
  platform: string | null;
  thirds: boolean;
  cross: boolean;
  grid: GridDivisions | null;
  aspect: AspectGuideId | null;
  /** The readout of the preview's zoom and the picture's pixel ratio. */
  pixelRatio: boolean;
  /** Edge and corner handles crop instead of scale. */
  cropMode: boolean;
}

export interface GuideActions {
  set(patch: Partial<GuideState>): void;
}

export const initialGuides: GuideState = {
  platform: null,
  thirds: false,
  cross: false,
  grid: null,
  aspect: null,
  pixelRatio: false,
  cropMode: false,
};

export const guideStore = createStore<GuideState & GuideActions>()((set) => ({
  ...initialGuides,
  set: (patch) => set(patch),
}));

export function useGuides<T>(selector: (state: GuideState & GuideActions) => T): T {
  return useStore(guideStore, selector);
}

/** A box as insets from the frame's edges, fractions of the width and the height. */
export interface Inset {
  readonly top: number;
  readonly right: number;
  readonly bottom: number;
  readonly left: number;
}

/**
 * The aspect-ratio frame guide inside a canvas of ratio `canvasRatio`: the largest centred box of
 * ratio `ratio` that fits.
 */
export function aspectFrame(canvasRatio: number, ratio: number): Inset {
  if (ratio >= canvasRatio) {
    const h = canvasRatio / ratio;
    return { top: (1 - h) / 2, bottom: (1 - h) / 2, left: 0, right: 0 };
  }
  const w = ratio / canvasRatio;
  return { left: (1 - w) / 2, right: (1 - w) / 2, top: 0, bottom: 0 };
}
