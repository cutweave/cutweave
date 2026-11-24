// SPDX-License-Identifier: GPL-3.0-or-later
// The one-time move of the theme and the saving preferences out of the view store's
// `localStorage` blob (`cutweave.view`) into the preferences store (WP-1M.11). Before this the
// theme and WP-1M.10's autosave rows lived there; a profile that only has them there keeps its
// choices. The values are copied into the new store first and removed from the blob only after
// main accepted them, so a failed write is simply tried again at the next start.

import {
  AUTOSAVE_BACKUPS,
  AUTOSAVE_INTERVALS_S,
  type PreferenceChanges,
} from "@cutweave/preferences";
import { VIEW_STORAGE_KEY } from "../state/viewStore.ts";
import type { PreferencesClient } from "./prefsStore.ts";

/** The slice of `Storage` the migration uses. */
export interface LegacyStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** The theme and autosave values the old blob held, validated; empty when there are none. */
export function legacyPreferences(blob: unknown): PreferenceChanges {
  const out: Record<string, unknown> = {};
  if (typeof blob !== "object" || blob === null) return out;
  const state = (blob as { state?: unknown }).state;
  if (typeof state !== "object" || state === null) return out;
  const { theme, autosave } = state as Record<string, unknown>;
  // The old theme choices were dark and light.
  if (theme === "dark" || theme === "light") out["appearance.theme"] = theme;
  if (typeof autosave === "object" && autosave !== null) {
    const a = autosave as Record<string, unknown>;
    if (typeof a.enabled === "boolean") out["saving.autosave"] = a.enabled;
    if ((AUTOSAVE_INTERVALS_S as readonly unknown[]).includes(a.intervalS)) {
      out["saving.interval"] = a.intervalS;
    }
    if ((AUTOSAVE_BACKUPS as readonly unknown[]).includes(a.keepBackups)) {
      out["saving.backups"] = a.keepBackups;
    }
  }
  return out as PreferenceChanges;
}

/**
 * Moves the old values into the preferences store, then removes them from the blob. A value
 * the person has already changed in the new store (it is no longer the default) is kept.
 * Returns the keys that moved. Never throws: the editor starts either way.
 */
export async function migrateLegacyView(
  storage: LegacyStorage | null,
  client: PreferencesClient,
): Promise<string[]> {
  if (storage === null || !client.persistent) return [];
  let blob: { state?: Record<string, unknown> } & Record<string, unknown>;
  try {
    const text = storage.getItem(VIEW_STORAGE_KEY);
    if (text === null) return [];
    blob = JSON.parse(text) as typeof blob;
  } catch {
    return [];
  }
  const found = legacyPreferences(blob);
  const state = blob.state;
  if (state === undefined || typeof state !== "object" || state === null) return [];
  const hasLegacy = "theme" in state || "autosave" in state;
  if (!hasLegacy) return [];
  const { defaults, values } = client.store.getState();
  const moved: string[] = [];
  try {
    for (const [key, value] of Object.entries(found)) {
      const k = key as keyof typeof values;
      if (value === defaults[k] || values[k] !== defaults[k]) continue;
      await client.set(k, value as never);
      moved.push(key);
    }
    delete state.theme;
    delete state.autosave;
    storage.setItem(VIEW_STORAGE_KEY, JSON.stringify(blob));
  } catch {
    // Main refused a value or storage is unavailable: keep the blob, try again at the next start.
  }
  return moved;
}
