// SPDX-License-Identifier: GPL-3.0-or-later
// The editor's view of the app preferences (WP-1M.11, arch §16). Electron main owns the file
// (`preferences.json`) and validates every write against the shared schema; this store holds the
// snapshot main sent before first paint, follows `prefs:changed`, and writes one key at a time.
// Without the desktop shell (plain `vite` dev, Vitest) it is an in-memory store with the schema's
// defaults, so every consumer works the same.

import {
  defaultPreferences,
  PREFERENCE_KEYS,
  type PreferenceChanges,
  type PreferenceKey,
  type PreferenceMap,
  validatePreference,
} from "@cutweave/preferences";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { PrefsBridge, PrefsExportResult, PrefsImportResult } from "../session/desktop.ts";

export interface PrefsState {
  values: PreferenceMap;
  /** Defaults on this machine (the new-project folder depends on it). */
  defaults: PreferenceMap;
  /** A snapshot from main arrived, or there is no desktop shell and the defaults stand. */
  loaded: boolean;
  /** Preferences are kept by the desktop shell (false in plain `vite` dev: they last a session). */
  persistent: boolean;
}

export function createPrefsStore() {
  const defaults = defaultPreferences();
  return createStore<PrefsState>()(() => ({
    values: defaults,
    defaults,
    loaded: false,
    persistent: false,
  }));
}

export type PrefsStore = ReturnType<typeof createPrefsStore>;

/**
 * Reads and writes the preferences. One per window; `attach` connects it to main.
 * Writes are optimistic: the store changes at once, and a write main refuses is rolled back from
 * main's own snapshot.
 */
export class PreferencesClient {
  readonly store: PrefsStore;
  #bridge: PrefsBridge | null = null;
  #off: (() => void) | null = null;

  constructor(store: PrefsStore = createPrefsStore()) {
    this.store = store;
  }

  /** Whether a desktop shell keeps the preferences. */
  get persistent(): boolean {
    return this.#bridge !== null;
  }

  /**
   * Loads main's snapshot and follows its changes; resolves once the store holds the values the
   * first paint should use. A bridge that fails leaves the defaults in place (never blocks).
   */
  async attach(bridge: PrefsBridge | null): Promise<void> {
    this.detach();
    this.#bridge = bridge;
    if (bridge === null) {
      this.store.setState({ loaded: true, persistent: false });
      return;
    }
    this.#off = bridge.onChanged((changes) => this.#apply(changes));
    try {
      const snapshot = await bridge.get();
      this.store.setState({
        values: { ...defaultPreferences(), ...snapshot.values },
        defaults: { ...defaultPreferences(), ...snapshot.defaults },
        loaded: true,
        persistent: true,
      });
    } catch {
      this.store.setState({ loaded: true, persistent: true });
    }
  }

  detach(): void {
    this.#off?.();
    this.#off = null;
    this.#bridge = null;
  }

  get<K extends PreferenceKey>(key: K): PreferenceMap[K] {
    return this.store.getState().values[key];
  }

  /** Changes one preference. Rejects when main refuses it (the value then goes back). */
  async set<K extends PreferenceKey>(key: K, value: PreferenceMap[K]): Promise<void> {
    const checked = validatePreference(key, value);
    if (!checked.ok) throw new Error(`invalid preference ${key}: ${checked.reason}`);
    this.#apply({ [key]: value } as PreferenceChanges);
    await this.#write(() => this.#bridge?.set(key, value));
  }

  /** Puts these keys back to their defaults. */
  async reset(keys: readonly PreferenceKey[]): Promise<void> {
    const { defaults } = this.store.getState();
    const changes: Record<string, unknown> = {};
    for (const key of keys) changes[key] = defaults[key];
    this.#apply(changes as PreferenceChanges);
    await this.#write(() => this.#bridge?.reset(keys));
  }

  /** "Reset all settings": every key. */
  resetAll(): Promise<void> {
    return this.reset(PREFERENCE_KEYS);
  }

  /** Asks main to write the app preferences to a file the person picks. */
  async exportToFile(only?: "shortcuts"): Promise<PrefsExportResult> {
    return this.#bridge?.exportToFile(only) ?? { status: "cancelled" };
  }

  /** Asks main to apply the valid keys of a file the person picks. */
  async importFromFile(only?: "shortcuts"): Promise<PrefsImportResult> {
    return this.#bridge?.importFromFile(only) ?? { status: "cancelled" };
  }

  #apply(changes: PreferenceChanges): void {
    this.store.setState((s) => ({ values: { ...s.values, ...changes } }));
  }

  async #write(call: () => Promise<void> | undefined): Promise<void> {
    if (this.#bridge === null) return;
    try {
      await call();
    } catch (error) {
      await this.#resync();
      throw error;
    }
  }

  async #resync(): Promise<void> {
    try {
      const snapshot = await this.#bridge?.get();
      if (snapshot !== undefined) this.store.setState({ values: snapshot.values });
    } catch {
      // Main is unreachable: the optimistic value stays until the next change.
    }
  }
}

/** The window's preferences: `main.tsx` attaches it to the desktop shell before first paint. */
export const preferences = new PreferencesClient();
export const prefsStore = preferences.store;

export function usePrefs<T>(selector: (state: PrefsState) => T): T {
  return useStore(prefsStore, selector);
}

/** One preference's value, re-rendering when it changes. */
export function usePref<K extends PreferenceKey>(key: K): PreferenceMap[K] {
  return useStore(prefsStore, (s) => s.values[key]);
}
