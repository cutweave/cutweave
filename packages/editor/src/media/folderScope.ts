// SPDX-License-Identifier: GPL-3.0-or-later
// Which folders a search covers (ADR-0020): the engine's `folder_scope` (cutweave-ops, find.rs) in
// TypeScript, kept in its own file so `find.ts` and `folders.ts` do not import each other.
import type { MediaFilter } from "@cutweave/protocol";

/** What the folder rules need of a folder: its parent. A project's `folders` fits. */
export type FolderParents = Readonly<
  Record<string, { readonly parent?: string | null | undefined } | undefined>
>;

/** `root` and every folder below it. */
export function folderSubtree(folders: FolderParents, root: string): Set<string> {
  const children = new Map<string, string[]>();
  for (const [id, folder] of Object.entries(folders)) {
    const parent = folder?.parent ?? null;
    if (parent === null) continue;
    const list = children.get(parent);
    if (list === undefined) children.set(parent, [id]);
    else list.push(id);
  }
  const out = new Set<string>([root]);
  const queue = [root];
  for (let next = queue.pop(); next !== undefined; next = queue.pop()) {
    for (const child of children.get(next) ?? []) {
      if (out.has(child)) continue;
      out.add(child);
      queue.push(child);
    }
  }
  return out;
}

/**
 * The folders whose assets `filter` keeps (`null` inside: the top level), or `null` when it keeps
 * every folder. The engine's rule (`folder_scope` in cutweave-ops): without `folder` the whole
 * project, or the top level alone with `recursive: false`; with `folder` that folder alone, or its
 * subtree with `recursive: true`.
 */
export function folderScope(
  folders: FolderParents,
  filter: MediaFilter | null | undefined,
): Set<string | null> | null {
  const folder = filter?.folder ?? null;
  const recursive = filter?.recursive ?? folder === null;
  if (folder === null && recursive) return null;
  if (!recursive) return new Set([folder]);
  return new Set<string | null>(folderSubtree(folders, folder as string));
}
