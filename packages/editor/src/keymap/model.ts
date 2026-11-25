// SPDX-License-Identifier: GPL-3.0-or-later
// The keymap's model (WP-2H.05): a preset (a data file of bindings) plus the person's overrides,
// resolved for a platform into one lookup table. Pure and synchronous: the app's singleton
// (`keymap/store.ts`) builds one from the preferences; tests build their own.
//
// A binding is `{command, args?, keys, when?}`: `keys` are alternatives (any of them runs it, the
// first is the one a menu shows), each a stroke or a chord of two. The overrides hold only the
// differences from the preset, binding id to the keys it now has (`[]` removes it).
import type { ShortcutOverrides, ShortcutPreset } from "@cutweave/preferences";
import { contextsOverlap, type KeyContext, keyCommand } from "./commands.ts";
import capcut from "./keymaps/capcut.json";
import nle from "./keymaps/nle.json";
import { bindingId, type PresetBinding, type PresetFile, parseBindingId } from "./preset.ts";
import { type KeyCombo, type KeyPlatform, parseKeys, spell, strokeKey } from "./stroke.ts";

/** The presets: data files, `keymaps/nle.json` (the default) and `keymaps/capcut.json`. */
export { bindingId, type PresetBinding, type PresetFile, parseBindingId };

export const PRESETS: Readonly<Record<ShortcutPreset, PresetFile>> = {
  nle: nle as PresetFile,
  capcut: capcut as PresetFile,
};

/** A binding as it is now: the preset's, changed by the person's overrides. */
export interface KeyBinding {
  /** `bindingId(command, args)`. */
  readonly id: string;
  readonly command: string;
  readonly args?: Readonly<Record<string, string>>;
  readonly context: KeyContext;
  /** The alternatives, in the order a menu prefers them. May be empty (removed). */
  readonly keys: readonly KeyCombo[];
  readonly unverified: boolean;
  /** Its keys cannot be changed (the text fields' and dialogs' own). */
  readonly fixed: boolean;
}

/** How the person's bindings differ from the preset: stored, and what the editor reads. */
export type Spelled = Readonly<Record<string, readonly string[]>>;

/** Why an assignment was refused or needs an answer. */
export interface Conflict {
  /** The binding that already has these keys. */
  readonly binding: KeyBinding;
  readonly keys: string;
}

function contextOf(command: string, when?: KeyContext): KeyContext {
  return when ?? keyCommand(command)?.context ?? "global";
}

export class KeymapModel {
  readonly preset: ShortcutPreset;
  readonly platform: KeyPlatform;
  readonly #overrides: ShortcutOverrides;
  readonly #presetSpelled: Map<string, string[]>;
  readonly #byId = new Map<string, KeyBinding>();
  /** First stroke → bindings that start with it. */
  readonly #index = new Map<string, KeyBinding[]>();
  readonly #unverified = new Set<string>();

  constructor(preset: ShortcutPreset, overrides: ShortcutOverrides, platform: KeyPlatform) {
    this.preset = preset;
    this.platform = platform;
    this.#overrides = overrides;
    const file = PRESETS[preset];
    const presetBindings = new Map<string, PresetBinding>();
    this.#presetSpelled = new Map();
    for (const entry of file.bindings) {
      if (entry.platforms !== undefined && !entry.platforms.includes(platform)) continue;
      const id = bindingId(entry.command, entry.args);
      presetBindings.set(id, entry);
      this.#presetSpelled.set(id, [...entry.keys]);
      if (entry.unverified === true) this.#unverified.add(id);
    }
    const ids = new Set([...presetBindings.keys(), ...Object.keys(overrides)]);
    for (const id of ids) {
      const base = presetBindings.get(id);
      const named = parseBindingId(id);
      const command = base?.command ?? named.command;
      const args = base?.args ?? named.args;
      const spelled = overrides[id] ?? base?.keys ?? [];
      const keys: KeyCombo[] = [];
      for (const text of spelled) {
        const combo = parseKeys(text, platform);
        if (combo !== null) keys.push(combo);
      }
      const def = keyCommand(command);
      const binding: KeyBinding = {
        id,
        command,
        ...(args !== undefined && { args }),
        context: contextOf(command, base?.when),
        keys,
        unverified: base?.unverified === true && overrides[id] === undefined,
        fixed: def?.fixed === true,
      };
      this.#byId.set(id, binding);
      for (const combo of keys) {
        const slot = strokeKey(combo);
        const list = this.#index.get(slot);
        if (list === undefined) this.#index.set(slot, [binding]);
        else if (!list.includes(binding)) list.push(binding);
      }
    }
  }

  /** Every binding with its current keys (removed ones have none). */
  all(): readonly KeyBinding[] {
    return [...this.#byId.values()];
  }

  get(id: string): KeyBinding | undefined {
    return this.#byId.get(id);
  }

  /** The keys of `id` as they are now, in menu order. */
  keysOf(id: string): readonly KeyCombo[] {
    return this.#byId.get(id)?.keys ?? [];
  }

  /**
   * The bindings whose keys start with `stroke` and whose context is one of `contexts`. A chord's
   * first stroke finds the chord; `second` then picks the one it completes.
   */
  lookup(stroke: KeyCombo, contexts: readonly KeyContext[]): readonly KeyBinding[] {
    const list = this.#index.get(strokeKey(stroke));
    if (list === undefined) return [];
    return list.filter((b) => contexts.includes(b.context));
  }

  /** What the person's keys are, spelled, for every binding that differs or exists. */
  effective(): Record<string, string[]> {
    const out: Record<string, string[]> = {};
    for (const binding of this.#byId.values()) out[binding.id] = binding.keys.map(spell);
    return out;
  }

  /** The preset's own keys for `id`, spelled (`[]` when the preset has none). */
  presetKeys(id: string): readonly string[] {
    return this.#presetSpelled.get(id) ?? [];
  }

  /** Whether `id` differs from the preset. */
  isChanged(id: string): boolean {
    return this.#overrides[id] !== undefined;
  }

  /** Whether the preset marks `id` as a best guess, and the person has not changed it. */
  isUnverified(id: string): boolean {
    return this.#byId.get(id)?.unverified === true;
  }

  /**
   * The bindings that already use `keys` (spelled) in a context that overlaps `id`'s. A key that is
   * the first stroke of a chord, or a chord starting with it, clashes too, since pressing it
   * would be ambiguous. Fixed bindings (text fields, dialogs) are the reserved keys' business.
   */
  conflicts(id: string, keys: string): Conflict[] {
    const combo = parseKeys(keys, this.platform);
    if (combo === null) return [];
    const mine = this.#byId.get(id);
    const context = mine?.context ?? contextOf(parseBindingId(id).command);
    const out: Conflict[] = [];
    for (const other of this.#byId.values()) {
      if (other.id === id || other.fixed) continue;
      if (!contextsOverlap(context, other.context)) continue;
      for (const theirs of other.keys) {
        if (clash(combo, theirs)) {
          out.push({ binding: other, keys: spell(theirs) });
          break;
        }
      }
    }
    return out;
  }
}

/** Whether pressing `a` and pressing `b` would be one ambiguous sequence. */
function clash(a: KeyCombo, b: KeyCombo): boolean {
  if (strokeKey(a) !== strokeKey(b)) return false;
  // Same first stroke: two plain strokes, or two chords with the same second stroke, are equal;
  // a stroke and a chord that starts with it block each other.
  if (a.next === undefined || b.next === undefined) return true;
  return spell(a.next) === spell(b.next);
}

// ---- editing: pure functions over the spelled keys ---------------------------------------------

/** The overrides that turn the preset into `wanted`: only the bindings that differ are kept. */
export function overridesFor(
  model: Pick<KeymapModel, "presetKeys">,
  wanted: Spelled,
): ShortcutOverrides {
  const out: ShortcutOverrides = {};
  for (const [id, keys] of Object.entries(wanted)) {
    const base = model.presetKeys(id);
    if (keys.length === base.length && keys.every((k, i) => k === base[i])) continue;
    out[id] = [...keys];
  }
  return out;
}

/**
 * `wanted` with `keys` added to `id` as its first (the one menus show). Giving a binding a key it
 * already has just moves that key first. `reassign` takes the key away from the bindings that
 * had it, as the person confirmed.
 */
export function withKey(
  model: KeymapModel,
  wanted: Spelled,
  id: string,
  keys: string,
  reassign: readonly Conflict[] = [],
): Spelled {
  const next: Record<string, readonly string[]> = { ...wanted };
  for (const lost of reassign) {
    next[lost.binding.id] = (next[lost.binding.id] ?? []).filter((k) => k !== lost.keys);
  }
  const current = next[id] ?? model.get(id)?.keys.map(spell) ?? [];
  next[id] = [keys, ...current.filter((k) => k !== keys)];
  return next;
}

/** `wanted` without `keys` on `id`. */
export function withoutKey(wanted: Spelled, id: string, keys: string): Spelled {
  return { ...wanted, [id]: (wanted[id] ?? []).filter((k) => k !== keys) };
}

/** `wanted` with `id` back to the preset's keys. */
export function withPresetKeys(model: KeymapModel, wanted: Spelled, id: string): Spelled {
  return { ...wanted, [id]: [...model.presetKeys(id)] };
}
