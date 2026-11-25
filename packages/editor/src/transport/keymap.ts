// SPDX-License-Identifier: GPL-3.0-or-later
// The transport's keys (WP-1M.04 interface): the handlers that carry the actions out, registered
// by whoever owns them (the transport bar registers its actions). One window `keydown` listener
// dispatches (`useKeymapListener`, mounted in the shell). Which key runs which action is the app
// keymap's (`keymap/`, WP-2H.05): the NLE preset has J/K/L, Space and the arrows, and a person's
// own bindings apply here too. A `Keymap` made with an explicit table (tests) reads that table.
//
// Keys stay with whatever already uses them: text fields, a focused control that acts on the
// key itself (Space on a button, arrows on a slider or radio group), and anything a handler
// deeper in the page already handled (`defaultPrevented`, e.g. the timeline's own keys and the
// toolbar's roving focus).
import { useEffect } from "react";
import { comboText, matchEvent, strokesOf } from "../keymap/store.ts";
import type { KeyCombo } from "../keymap/stroke.ts";

export type ActionId =
  | "transport:togglePlay"
  | "transport:reverse"
  | "transport:pause"
  | "transport:forward"
  | "transport:stepBack"
  | "transport:stepForward";

/** Key (as {@link keyOf} names it) → action. */
export type KeyBindings = Readonly<Record<string, ActionId>>;

/** The NLE preset's transport keys, as an explicit table for a `Keymap` that needs one. */
export const DEFAULT_KEYMAP: KeyBindings = {
  Space: "transport:togglePlay",
  J: "transport:reverse",
  K: "transport:pause",
  L: "transport:forward",
  ArrowLeft: "transport:stepBack",
  ArrowRight: "transport:stepForward",
};

/** Actions that act once per press: holding the key does not repeat them. */
const NO_REPEAT: ReadonlySet<ActionId> = new Set([
  "transport:togglePlay",
  "transport:reverse",
  "transport:pause",
  "transport:forward",
]);

/** The keymap command of each action (`keymap/commands.ts`). */
export const ACTION_COMMANDS: Readonly<Record<ActionId, string>> = {
  "transport:togglePlay": "transport.toggle_play",
  "transport:reverse": "transport.reverse",
  "transport:pause": "transport.pause",
  "transport:forward": "transport.forward",
  "transport:stepBack": "transport.step_back",
  "transport:stepForward": "transport.step_forward",
};

const ACTION_OF = new Map(
  (Object.entries(ACTION_COMMANDS) as [ActionId, string][]).map(([action, id]) => [id, action]),
);

/** The slice of a `KeyboardEvent` the keymap reads. */
export interface KeyEventLike {
  readonly key: string;
  readonly code?: string;
  readonly isComposing?: boolean;
  readonly keyCode?: number;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
  readonly repeat: boolean;
  readonly defaultPrevented: boolean;
  readonly target: EventTarget | null;
}

/** `Ctrl+Alt+Shift+Meta+<key>`; letters upper-case, the space bar `Space`. */
export function keyOf(
  e: Pick<KeyEventLike, "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">,
): string {
  const key = e.key === " " ? "Space" : e.key.length === 1 ? e.key.toUpperCase() : e.key;
  const mods = [
    e.ctrlKey ? "Ctrl" : null,
    e.altKey ? "Alt" : null,
    e.shiftKey ? "Shift" : null,
    e.metaKey ? "Meta" : null,
  ].filter((m) => m !== null);
  return [...mods, key].join("+");
}

/** How tooltips show a key. */
const KEY_LABELS: Readonly<Record<string, string>> = {
  ArrowLeft: "←",
  ArrowRight: "→",
  ArrowUp: "↑",
  ArrowDown: "↓",
};

interface TargetLike {
  readonly tagName?: unknown;
  readonly isContentEditable?: unknown;
  getAttribute?(name: string): string | null;
}

const TEXT_TAGS = new Set(["INPUT", "TEXTAREA", "SELECT"]);
/** Controls that press on Space. */
const SPACE_ROLES = new Set(["button", "checkbox", "radio", "switch", "menuitem", "tab", "option"]);
const SPACE_TAGS = new Set(["BUTTON", "A", "SUMMARY"]);
/** Controls that move on the arrow keys. */
const ARROW_ROLES = new Set([
  "slider",
  "radio",
  "tab",
  "menuitem",
  "option",
  "listbox",
  "spinbutton",
]);

/** Whether the focused element uses `key` itself. */
function ownedByTarget(target: EventTarget | null, key: string): boolean {
  const el = target as TargetLike | null;
  if (el === null || typeof el.tagName !== "string") return false;
  if (el.isContentEditable === true || TEXT_TAGS.has(el.tagName)) return true;
  const role = el.getAttribute?.("role") ?? "";
  if (key === "Space") return SPACE_TAGS.has(el.tagName) || SPACE_ROLES.has(role);
  if (key.startsWith("Arrow")) return ARROW_ROLES.has(role);
  return false;
}

/** A combo as `keyOf` spells it (`Ctrl+Alt+Shift+Meta+<key>`). */
function keyOfCombo(combo: KeyCombo): string {
  const mods = [
    combo.mod === true || combo.ctrl === true ? "Ctrl" : null,
    combo.alt === true ? "Alt" : null,
    combo.shift === true ? "Shift" : null,
  ].filter((m) => m !== null);
  return [...mods, combo.key].join("+");
}

export class Keymap {
  /** An explicit table (tests); without one the app keymap decides. */
  readonly #bindings: KeyBindings | undefined;
  /** Handlers per action, newest last (the newest one runs). */
  readonly #handlers = new Map<ActionId, (() => void)[]>();

  constructor(bindings?: KeyBindings) {
    this.#bindings = bindings;
  }

  /** Runs `handler` for `action` until the returned function is called. */
  register(action: ActionId, handler: () => void): () => void {
    const list = this.#handlers.get(action) ?? [];
    list.push(handler);
    this.#handlers.set(action, list);
    return () => {
      const current = this.#handlers.get(action);
      if (current === undefined) return;
      const i = current.lastIndexOf(handler);
      if (i >= 0) current.splice(i, 1);
      if (current.length === 0) this.#handlers.delete(action);
    };
  }

  /** The action `e` triggers, or `null`. */
  actionFor(e: KeyEventLike): ActionId | null {
    if (this.#bindings !== undefined) return this.#bindings[keyOf(e)] ?? null;
    const binding = matchEvent(e, ["playback"]);
    return binding === null ? null : (ACTION_OF.get(binding.command) ?? null);
  }

  /** Dispatches a key press; true when an action ran (the caller prevents the default). */
  handle(e: KeyEventLike): boolean {
    if (e.defaultPrevented) return false;
    const action = this.actionFor(e);
    if (action === null) return false;
    if (ownedByTarget(e.target, keyOf(e))) return false;
    const handler = this.#handlers.get(action)?.at(-1);
    if (handler === undefined) return false;
    if (e.repeat && NO_REPEAT.has(action)) return true;
    handler();
    return true;
  }

  /** Every binding, key (as {@link keyOf} names it) and action: the table `handle` reads. */
  entries(): readonly (readonly [string, ActionId])[] {
    if (this.#bindings !== undefined) return Object.entries(this.#bindings);
    const out: (readonly [string, ActionId])[] = [];
    for (const [action, id] of Object.entries(ACTION_COMMANDS) as [ActionId, string][]) {
      for (const combo of strokesOf(id)) out.push([keyOfCombo(combo), action]);
    }
    return out;
  }

  /** The keys bound to `action`, as tooltips show them (`Space`, `J`, `←`). */
  keysFor(action: ActionId): string[] {
    if (this.#bindings !== undefined) {
      return Object.entries(this.#bindings)
        .filter(([, a]) => a === action)
        .map(([key]) => KEY_LABELS[key] ?? key);
    }
    return strokesOf(ACTION_COMMANDS[action]).map((combo) => comboText(combo));
  }
}

/** The app's keymap. */
export const keymap = new Keymap();

/** Mount once (the shell): window key presses go through {@link keymap}. */
export function useKeymapListener(map: Keymap = keymap): void {
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (map.handle(event)) event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [map]);
}

/** Registers `handler` for `action` while the calling component is mounted. */
export function useKeyAction(action: ActionId, handler: () => void, map: Keymap = keymap): void {
  useEffect(() => map.register(action, handler), [map, action, handler]);
}
