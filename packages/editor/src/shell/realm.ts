// SPDX-License-Identifier: GPL-3.0-or-later
// Realm-safe DOM checks (WP-2H.15). A detached panel's window is a second JavaScript realm: an
// event fired there, or a node a library made with that window's document, is not an instance of
// the main window's `Element` or `HTMLInputElement`, so `x instanceof HTMLElement` is false for it.
// These check what the node is instead (its node type and tag), in any window.

/** Whether `value` is a DOM element, from any window. */
export function isElement(value: unknown): value is Element {
  return typeof value === "object" && value !== null && (value as Node).nodeType === 1;
}

/** Whether `value` is an HTML element (it has `dataset` and `focus`), from any window. */
export function isHTMLElement(value: unknown): value is HTMLElement {
  return (
    isElement(value) &&
    "dataset" in value &&
    typeof (value as Partial<HTMLElement>).focus === "function"
  );
}

/** Whether `value` is an element with this tag (`input`, `textarea`...), from any window. */
export function hasTag<K extends keyof HTMLElementTagNameMap>(
  value: unknown,
  tag: K,
): value is HTMLElementTagNameMap[K] {
  return isElement(value) && value.tagName === tag.toUpperCase();
}
