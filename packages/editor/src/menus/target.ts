// SPDX-License-Identifier: GPL-3.0-or-later
// The selection and target rules of every context menu (WP-2H.12, ADR-0026, research §5.4):
//   * a right-click on an item that is not selected selects it first (`view.select`, not an undo
//     step), and the menu acts on that item;
//   * on a selected item the selection is kept and the menu acts on all of it;
//   * on a place with nothing under it (an empty timeline lane) the selection is kept; the bin
//     clears it (the Explorer convention). Each surface says which, in its selection adapter.
import type { MenuTarget, SelectionAdapter } from "./types.ts";

export interface ResolvedTarget {
  readonly target: MenuTarget;
  /** The selection to set before the menu opens, or `null` to leave it. */
  readonly select: readonly string[] | null;
}

/** Pure: what the target becomes and whether the selection changes. */
export function resolveTarget(
  target: MenuTarget,
  adapter: SelectionAdapter | undefined,
): ResolvedTarget {
  if (adapter === undefined) return { target, select: null };
  const selected = adapter.current();
  const hit = target.ids;

  if (hit.length === 0) {
    // A place, not an item.
    return {
      target,
      select: adapter.empty === "clear" && selected.length > 0 ? [] : null,
    };
  }
  const inSelection = hit.every((id) => selected.includes(id));
  if (inSelection) {
    // The menu acts on the whole selection, in selection order.
    return { target: { ...target, ids: [...selected] }, select: null };
  }
  return { target, select: [...hit] };
}

/** Applies {@link resolveTarget} and returns the target the menu is built for. */
export function applyTargetRules(
  target: MenuTarget,
  adapter: SelectionAdapter | undefined,
): MenuTarget {
  const { target: resolved, select } = resolveTarget(target, adapter);
  if (select !== null) adapter?.set(select);
  return resolved;
}
