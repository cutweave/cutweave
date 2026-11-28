// SPDX-License-Identifier: GPL-3.0-or-later
//! The RFC 6902 subset the engine emits (arch §5.2, ADR-0005): `add`, `remove`, `replace` and
//! `move`, where `remove` and `replace` carry the expected old value (`old`). Mirrors
//! `apply_patch_json` in `cutweave-commands`, so the UI's JSON copy of the document follows the
//! engine's exactly.

import type { PatchOp } from "./generated/types.ts";

/** A JSON value. Flicks are plain numbers on the wire (ADR-0015), so no `bigint`. */
export type Json =
  | null
  | boolean
  | number
  | string
  | readonly Json[]
  | { readonly [key: string]: Json };

/** A patch does not apply to the document it was given. */
export class PatchError extends Error {
  readonly opIndex: number;

  constructor(opIndex: number, message: string) {
    super(`op ${opIndex}: ${message}`);
    this.name = "PatchError";
    this.opIndex = opIndex;
  }
}

function unescapeToken(token: string): string {
  return token.replaceAll("~1", "/").replaceAll("~0", "~");
}

function parsePointer(pointer: string): string[] {
  if (pointer === "") {
    return [];
  }
  if (!pointer.startsWith("/")) {
    throw new Error(`bad JSON Pointer \`${pointer}\``);
  }
  return pointer.slice(1).split("/").map(unescapeToken);
}

function isObject(value: Json | undefined): value is { readonly [key: string]: Json } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isArray(value: Json | undefined): value is readonly Json[] {
  return Array.isArray(value);
}

function arrayIndex(token: string, max: number): number {
  if (!/^(0|[1-9][0-9]*)$/.test(token)) {
    throw new Error(`bad array index \`${token}\``);
  }
  const index = Number(token);
  if (index > max) {
    throw new Error(`array index ${index} out of range`);
  }
  return index;
}

/** Structural equality (numbers by value, object keys unordered). */
export function jsonEqual(a: Json, b: Json): boolean {
  if (a === b) {
    return true;
  }
  if (isArray(a)) {
    return isArray(b) && a.length === b.length && a.every((x, i) => jsonEqual(x, b[i] as Json));
  }
  if (isObject(a)) {
    if (!isObject(b)) {
      return false;
    }
    const keys = Object.keys(a);
    return (
      keys.length === Object.keys(b).length &&
      keys.every((k) => Object.hasOwn(b, k) && jsonEqual(a[k] as Json, b[k] as Json))
    );
  }
  return false;
}

/** Reads the value at `tokens`, or throws when a step is missing. */
function get(doc: Json, tokens: readonly string[]): Json {
  let node: Json | undefined = doc;
  for (const token of tokens) {
    if (isObject(node) && Object.hasOwn(node, token)) {
      node = node[token];
    } else if (isArray(node)) {
      node = node[arrayIndex(token, node.length - 1)];
    } else {
      throw new Error(`path does not exist (at \`${token}\`)`);
    }
    if (node === undefined) {
      throw new Error(`path does not exist (at \`${token}\`)`);
    }
  }
  return node;
}

/**
 * The containers one `applyPatch` call made: copies of what the document held, reachable from
 * the new document only, so a later op of the same patch changes them in place. Without it
 * every op copied the path from the root again, and a patch of 2,000 ops on the clips of one
 * track copied that track's 2,000-entry map 2,000 times (issue #231).
 */
type Made = Set<object>;

/** How often `applyPatch` copied a container; tests count copies instead of timing them. */
export const patchWork = { copies: 0 };

type Edit = (parent: Json, key: string, made: Made) => Json;

/** `node` when this call made it, else a copy of it that is this call's own. */
function own<T extends object>(node: T, made: Made): T {
  if (made.has(node)) {
    return node;
  }
  const copy = (Array.isArray(node) ? [...node] : { ...node }) as T;
  patchWork.copies += 1;
  made.add(copy);
  return copy;
}

/** Copies the spine down to the parent of `tokens` (once per patch) and lets `edit` change it. */
function update(doc: Json, tokens: readonly string[], at: number, edit: Edit, made: Made): Json {
  const head = tokens[at];
  if (head === undefined) {
    throw new Error("the root cannot be edited");
  }
  if (at === tokens.length - 1) {
    return edit(doc, head, made);
  }
  if (isObject(doc) && Object.hasOwn(doc, head)) {
    const child = update(doc[head] as Json, tokens, at + 1, edit, made);
    const copy = own(doc, made) as { [key: string]: Json };
    copy[head] = child;
    return copy;
  }
  if (isArray(doc)) {
    const index = arrayIndex(head, doc.length - 1);
    const child = update(doc[index] as Json, tokens, at + 1, edit, made);
    const copy = own(doc, made) as Json[];
    copy[index] = child;
    return copy;
  }
  throw new Error(`path does not exist (at \`${head}\`)`);
}

const addAt =
  (value: Json): Edit =>
  (parent, key, made) => {
    if (isObject(parent)) {
      if (Object.hasOwn(parent, key)) {
        throw new Error(`\`${key}\` already exists`);
      }
      const copy = own(parent, made) as { [key: string]: Json };
      copy[key] = value;
      return copy;
    }
    if (isArray(parent)) {
      const index = arrayIndex(key, parent.length);
      const copy = own(parent, made) as Json[];
      copy.splice(index, 0, value);
      return copy;
    }
    throw new Error("parent is not a container");
  };

const removeAt: Edit = (parent, key, made) => {
  if (isObject(parent) && Object.hasOwn(parent, key)) {
    const copy = own(parent, made) as { [key: string]: Json };
    delete copy[key];
    return copy;
  }
  if (isArray(parent)) {
    const index = arrayIndex(key, parent.length - 1);
    const copy = own(parent, made) as Json[];
    copy.splice(index, 1);
    return copy;
  }
  throw new Error(`\`${key}\` does not exist`);
};

const replaceAt =
  (value: Json): Edit =>
  (parent, key, made) => {
    if (isObject(parent) && Object.hasOwn(parent, key)) {
      const copy = own(parent, made) as { [key: string]: Json };
      copy[key] = value;
      return copy;
    }
    if (isArray(parent)) {
      const index = arrayIndex(key, parent.length - 1);
      const copy = own(parent, made) as Json[];
      copy[index] = value;
      return copy;
    }
    throw new Error(`\`${key}\` does not exist`);
  };

function applyOp(doc: Json, op: PatchOp, made: Made): Json {
  switch (op.op) {
    case "add":
      return update(doc, parsePointer(op.path), 0, addAt(op.value), made);
    case "remove": {
      const tokens = parsePointer(op.path);
      if (!jsonEqual(get(doc, tokens), op.old)) {
        throw new Error(`value at \`${op.path}\` differs from the expected one`);
      }
      return update(doc, tokens, 0, removeAt, made);
    }
    case "replace": {
      const tokens = parsePointer(op.path);
      if (!jsonEqual(get(doc, tokens), op.old)) {
        throw new Error(`value at \`${op.path}\` differs from the expected one`);
      }
      return update(doc, tokens, 0, replaceAt(op.value), made);
    }
    case "move": {
      // Remove at `from`, then add at `path` resolved in the document after the removal.
      const from = parsePointer(op.from);
      const value = get(doc, from);
      return update(
        update(doc, from, 0, removeAt, made),
        parsePointer(op.path),
        0,
        addAt(value),
        made,
      );
    }
  }
}

/**
 * Applies `patch` to `doc` and returns the new document; `doc` is never mutated and unchanged
 * subtrees are shared (cheap for the docStore). Throws {@link PatchError} when an op does not
 * apply, e.g. its `old` value no longer matches.
 */
export function applyPatch<T extends Json>(doc: T, patch: readonly PatchOp[]): T {
  let current: Json = doc;
  const made: Made = new Set();
  patch.forEach((op, index) => {
    try {
      current = applyOp(current, op, made);
    } catch (error) {
      throw new PatchError(index, error instanceof Error ? error.message : String(error));
    }
  });
  return current as T;
}
