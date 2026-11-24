// SPDX-License-Identifier: GPL-3.0-or-later
// The keymap (WP-1M.04 interface): one map from keys to action IDs, and the handlers that carry
// the actions out, registered by whoever owns them (the transport bar registers its actions).
// One window `keydown` listener dispatches (`useKeymapListener`, mounted in the shell). v1 is a
// fixed map with the NLE defaults (arch §16: J/K/L, Space, arrows); rebinding and presets are P2,
// which only has to swap the map.
//
// Keys stay with whatever already uses them: text fields, a focused control that acts on the
// key itself (Space on a button, arrows on a slider or radio group), and anything a handler
// deeper in the page already handled (`defaultPrevented`, e.g. the timeline's own keys and the
// toolbar's roving focus).
import { useEffect } from "react";

export type ActionId =
  | "transport:togglePlay"
  | "transport:reverse"
  | "transport:pause"
  | "transport:forward"
  | "transport:stepBack"
  | "transport:stepForward";

/** Key (as {@link keyOf} names it) → action. */
export type KeyBindings = Readonly<Record<string, ActionId>>;

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

/** The slice of a `KeyboardEvent` the keymap reads. */
export interface KeyEventLike {
  readonly key: string;
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

export class Keymap {
  readonly #bindings: KeyBindings;
  /** Handlers per action, newest last (the newest one runs). */
  readonly #handlers = new Map<ActionId, (() => void)[]>();

  constructor(bindings: KeyBindings = DEFAULT_KEYMAP) {
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
    return this.#bindings[keyOf(e)] ?? null;
  }

  /** Dispatches a key press; true when an action ran (the caller prevents the default). */
  handle(e: KeyEventLike): boolean {
    if (e.defaultPrevented) return false;
    const key = keyOf(e);
    const action = this.#bindings[key];
    if (action === undefined) return false;
    if (ownedByTarget(e.target, key)) return false;
    const handler = this.#handlers.get(action)?.at(-1);
    if (handler === undefined) return false;
    if (e.repeat && NO_REPEAT.has(action)) return true;
    handler();
    return true;
  }

  /** The keys bound to `action`, as tooltips show them (`Space`, `J`, `←`). */
  keysFor(action: ActionId): string[] {
    return Object.entries(this.#bindings)
      .filter(([, a]) => a === action)
      .map(([key]) => KEY_LABELS[key] ?? key);
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
