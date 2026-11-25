// SPDX-License-Identifier: GPL-3.0-or-later
// The app's keymap (WP-2H.05): one model, built from the preferences (`shortcuts.preset` and
// `shortcuts.bindings`) and rebuilt the next time anything asks after they change. Every handler
// reads its keys here: the menu bar, the transport, the timeline, the bin, the source view and
// the global dispatcher. A lookup is a map read, well under 0.2 ms.
import type { ShortcutOverrides, ShortcutPreset } from "@cutweave/preferences";
import { preferences } from "../prefs/prefsStore.ts";
import type { KeyContext } from "./commands.ts";
import { type KeyBinding, KeymapModel } from "./model.ts";
import {
  type KeyCombo,
  type KeyEventLike,
  type KeyPlatform,
  parseKeys,
  spell,
  strokeKey,
  strokeOfEvent,
  strokeParts,
} from "./stroke.ts";

/** The platform the page runs on, as the keymap names it. */
export function currentKeyPlatform(): KeyPlatform {
  const name = typeof navigator === "undefined" ? "" : navigator.platform;
  if (/^mac|darwin/i.test(name)) return "mac";
  if (/^win/i.test(name)) return "windows";
  return "linux";
}

let platformOverride: KeyPlatform | null = null;

/** Tests pin the platform; `null` goes back to the page's own. */
export function setKeyPlatform(platform: KeyPlatform | null): void {
  platformOverride = platform;
}

interface Cached {
  readonly preset: ShortcutPreset;
  readonly overrides: ShortcutOverrides;
  readonly model: KeymapModel;
}

const cache = new Map<KeyPlatform, Cached>();

/** The keymap as the preferences have it now, for `platform` (the menus ask for the three). */
export function keymapModelFor(platform: KeyPlatform): KeymapModel {
  const values = preferences.store.getState().values;
  const preset = values["shortcuts.preset"];
  const overrides = values["shortcuts.bindings"];
  const known = cache.get(platform);
  if (known !== undefined && known.preset === preset && known.overrides === overrides) {
    return known.model;
  }
  const model = new KeymapModel(preset, overrides, platform);
  cache.set(platform, { preset, overrides, model });
  return model;
}

/** The keymap as the preferences have it now, for the platform the page runs on. */
export function keymapModel(): KeymapModel {
  return keymapModelFor(platformOverride ?? currentKeyPlatform());
}

/** Whether a press is part of an input method's composition: no shortcut may take it (IME safety). */
export function isComposing(e: Pick<KeyEventLike, "isComposing" | "keyCode">): boolean {
  return e.isComposing === true || e.keyCode === 229;
}

/**
 * The single-stroke binding a key press runs among `contexts`, or `null`. A letter matches by what
 * the key prints, or by the physical key when the layout prints a non-Latin letter.
 */
export function matchEvent(e: KeyEventLike, contexts: readonly KeyContext[]): KeyBinding | null {
  if (isComposing(e)) return null;
  const model = keymapModel();
  const stroke = strokeOfEvent(e, model.platform);
  if (stroke === null) return null;
  for (const binding of model.lookup(stroke, contexts)) {
    if (binding.keys.some((k) => k.next === undefined && spell(k) === spell(stroke))) {
      return binding;
    }
  }
  return null;
}

/** The first stroke of an event, for the chord state. */
export function strokeOf(e: KeyEventLike): KeyCombo | null {
  if (isComposing(e)) return null;
  return strokeOfEvent(e, keymapModel().platform);
}

/** The chords (among `contexts`) that start with `stroke`. */
export function chordsStartingWith(
  stroke: KeyCombo,
  contexts: readonly KeyContext[],
): readonly KeyBinding[] {
  return keymapModel()
    .lookup(stroke, contexts)
    .filter((b) => b.keys.some((k) => k.next !== undefined && strokeKey(k) === strokeKey(stroke)));
}

/** The binding a chord's second stroke completes, or `null`. */
export function completeChord(
  first: KeyCombo,
  second: KeyCombo,
  contexts: readonly KeyContext[],
): KeyBinding | null {
  const wanted = spell({ ...first, next: second });
  for (const binding of keymapModel().lookup(first, contexts)) {
    if (binding.keys.some((k) => spell(k) === wanted)) return binding;
  }
  return null;
}

/** The alternatives of a binding that a single press runs (a chord is not one press). */
export function strokesOf(id: string): readonly KeyCombo[] {
  return keymapModel()
    .keysOf(id)
    .filter((k) => k.next === undefined);
}

/** A combo as text: `Ctrl+Shift+S`; a chord `Ctrl+K Ctrl+S`. */
export function comboText(combo: KeyCombo, platform: KeyPlatform = keymapModel().platform): string {
  const first = strokeParts(combo, platform).join("+");
  return combo.next === undefined ? first : `${first} ${comboText(combo.next, platform)}`;
}

/** The key a menu or a tooltip shows for `id`: its first alternative, or `undefined`. */
export function hintOf(
  id: string,
  platform: KeyPlatform = keymapModel().platform,
): string | undefined {
  const first = keymapModel().keysOf(id)[0];
  return first === undefined ? undefined : comboText(first, platform);
}

/** A spelling read for `platform` (`Mod+K` as `Ctrl+K` or `⌘+K`); the text itself when invalid. */
export function spellingText(
  spelled: string,
  platform: KeyPlatform = keymapModel().platform,
): string {
  const combo = parseKeys(spelled, platform);
  return combo === null ? spelled : comboText(combo, platform);
}
