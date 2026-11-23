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

type Edit = (parent: Json, key: string) => Json;

/** Copies the spine down to the parent of `tokens` and lets `edit` change that parent. */
function update(doc: Json, tokens: readonly string[], edit: Edit): Json {
  const [head, ...rest] = tokens;
  if (head === undefined) {
    throw new Error("the root cannot be edited");
  }
  if (rest.length === 0) {
    return edit(doc, head);
  }
  if (isObject(doc) && Object.hasOwn(doc, head)) {
    return { ...doc, [head]: update(doc[head] as Json, rest, edit) };
  }
  if (isArray(doc)) {
    const index = arrayIndex(head, doc.length - 1);
    const copy = [...doc];
    copy[index] = update(doc[index] as Json, rest, edit);
    return copy;
  }
  throw new Error(`path does not exist (at \`${head}\`)`);
}

function addAt(parent: Json, key: string, value: Json): Json {
  if (isObject(parent)) {
    if (Object.hasOwn(parent, key)) {
      throw new Error(`\`${key}\` already exists`);
    }
    return { ...parent, [key]: value };
  }
  if (isArray(parent)) {
    const index = arrayIndex(key, parent.length);
    return [...parent.slice(0, index), value, ...parent.slice(index)];
  }
  throw new Error("parent is not a container");
}

function removeAt(parent: Json, key: string): Json {
  if (isObject(parent) && Object.hasOwn(parent, key)) {
    const { [key]: _removed, ...rest } = parent;
    return rest;
  }
  if (isArray(parent)) {
    const index = arrayIndex(key, parent.length - 1);
    return [...parent.slice(0, index), ...parent.slice(index + 1)];
  }
  throw new Error(`\`${key}\` does not exist`);
}

function replaceAt(parent: Json, key: string, value: Json): Json {
  if (isObject(parent) && Object.hasOwn(parent, key)) {
    return { ...parent, [key]: value };
  }
  if (isArray(parent)) {
    const index = arrayIndex(key, parent.length - 1);
    const copy = [...parent];
    copy[index] = value;
    return copy;
  }
  throw new Error(`\`${key}\` does not exist`);
}

function applyOp(doc: Json, op: PatchOp): Json {
  switch (op.op) {
    case "add":
      return update(doc, parsePointer(op.path), (p, k) => addAt(p, k, op.value));
    case "remove": {
      const tokens = parsePointer(op.path);
      if (!jsonEqual(get(doc, tokens), op.old)) {
        throw new Error(`value at \`${op.path}\` differs from the expected one`);
      }
      return update(doc, tokens, removeAt);
    }
    case "replace": {
      const tokens = parsePointer(op.path);
      if (!jsonEqual(get(doc, tokens), op.old)) {
        throw new Error(`value at \`${op.path}\` differs from the expected one`);
      }
      return update(doc, tokens, (p, k) => replaceAt(p, k, op.value));
    }
    case "move": {
      // Remove at `from`, then add at `path` resolved in the document after the removal.
      const from = parsePointer(op.from);
      const value = get(doc, from);
      return update(update(doc, from, removeAt), parsePointer(op.path), (p, k) =>
        addAt(p, k, value),
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
  patch.forEach((op, index) => {
    try {
      current = applyOp(current, op);
    } catch (error) {
      throw new PatchError(index, error instanceof Error ? error.message : String(error));
    }
  });
  return current as T;
}
