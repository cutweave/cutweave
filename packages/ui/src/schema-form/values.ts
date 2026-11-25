// SPDX-License-Identifier: GPL-3.0-or-later
/** Pure helpers on plain JSON values and dot paths. */

export function getPath(value: unknown, path: string): unknown {
  if (path === "") return value;
  let cur: unknown = value;
  for (const key of path.split(".")) {
    if (cur === null || typeof cur !== "object") return undefined;
    cur = (cur as Record<string, unknown>)[key];
  }
  return cur;
}

export function joinPath(parent: string, key: string | number): string {
  return parent === "" ? String(key) : `${parent}.${key}`;
}

export function deepEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  return ka.every((k) =>
    deepEqual((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]),
  );
}

/** A new value with `path` set (arrays and objects along the way are copied, not mutated). */
export function setPath(value: unknown, path: string, next: unknown): unknown {
  if (path === "") return next;
  const [head, ...rest] = path.split(".") as [string, ...string[]];
  const sub = rest.join(".");
  if (Array.isArray(value)) {
    const copy = value.slice();
    const index = Number(head);
    copy[index] = setPath(copy[index], sub, next);
    return copy;
  }
  const base = value !== null && typeof value === "object" ? value : {};
  return { ...base, [head]: setPath((base as Record<string, unknown>)[head], sub, next) };
}

/**
 * Folds the values of a multi-selection into one value plus the paths where the selection
 * disagrees (the `mixed` prop). Objects are compared leaf by leaf; arrays count as one leaf.
 * The merged value takes the first selection's value at a mixed path.
 */
export function mergeSelection(values: readonly unknown[]): { value: unknown; mixed: string[] } {
  const mixed: string[] = [];
  const first = values[0];
  const walk = (items: readonly unknown[], path: string): unknown => {
    const head = items[0];
    const allObjects = items.every((v) => v !== null && typeof v === "object" && !Array.isArray(v));
    if (allObjects) {
      const keys = new Set<string>();
      for (const v of items) for (const k of Object.keys(v as object)) keys.add(k);
      const out: Record<string, unknown> = {};
      for (const k of keys) {
        out[k] = walk(
          items.map((v) => (v as Record<string, unknown>)[k]),
          joinPath(path, k),
        );
      }
      return out;
    }
    if (!items.every((v) => deepEqual(v, head))) mixed.push(path);
    return head;
  };
  if (values.length === 0) return { value: undefined, mixed };
  return { value: values.length === 1 ? first : walk(values, ""), mixed };
}

export function humanize(key: string): string {
  const spaced = key
    .replace(/[_-]+/g, " ")
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .trim()
    .toLowerCase();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}
