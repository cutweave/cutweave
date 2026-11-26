// SPDX-License-Identifier: GPL-3.0-or-later
// Selection rules (timeline spec §7, INV-341/342): a click selects the whole linked group, Alt
// only the item; Shift adds or removes; a marquee selects what it touches and their groups.
// Commands receive one id per group (the engine expands groups itself); a selection that holds
// only some members of a group edits with `unlink: true`, or is refused where a command cannot.
import type { TimelineModel } from "../model.ts";
import { groupOf, itemOf } from "./doc.ts";

export interface SelectOptions {
  /** Alt: only the item itself. */
  readonly alt: boolean;
  /** The toolbar's linked selection. */
  readonly linked: boolean;
}

/** What a click on `id` selects: its group (clicked first), or only it. */
export function clickUnit(model: TimelineModel, id: string, o: SelectOptions): readonly string[] {
  return o.linked && !o.alt ? groupOf(model, id) : [id];
}

/** Shift+click: removes `unit` when all of it is selected, otherwise adds what is missing. */
export function toggled(selection: readonly string[], unit: readonly string[]): readonly string[] {
  if (unit.every((id) => selection.includes(id))) {
    return selection.filter((id) => !unit.includes(id));
  }
  return [...selection, ...unit.filter((id) => !selection.includes(id))];
}

/** A marquee's selection: the touched items and (unless Alt) their groups. */
export function marqueeSelection(
  model: TimelineModel,
  touched: readonly string[],
  o: SelectOptions & { readonly additive: boolean; readonly current: readonly string[] },
): readonly string[] {
  const out: string[] = o.additive ? [...o.current] : [];
  const seen = new Set(out);
  for (const id of touched) {
    if (itemOf(model, id) === undefined) continue;
    for (const m of clickUnit(model, id, o)) {
      if (!seen.has(m)) {
        seen.add(m);
        out.push(m);
      }
    }
  }
  return out;
}

export const sameIds = (a: readonly string[], b: readonly string[]): boolean =>
  a.length === b.length && a.every((id) => b.includes(id));

/** Whether `id`'s group has members both in and out of `selection`. */
export function isPartial(model: TimelineModel, selection: readonly string[], id: string): boolean {
  const members = groupOf(model, id);
  if (members.length < 2) return false;
  const inside = members.filter((m) => selection.includes(m)).length;
  return inside > 0 && inside < members.length;
}

export interface EditTargets {
  /** The ids to name in the command, in selection order. */
  readonly items: readonly string[];
  /** Some group is only partly selected: edit exactly these items (`unlink: true`). */
  readonly partial: boolean;
}

/**
 * The items an edit of `selection` names: one per linked group, or, when some group is only
 * partly selected, exactly the selected items (INV-342).
 */
export function editTargets(model: TimelineModel, selection: readonly string[]): EditTargets {
  const present = selection.filter((id) => itemOf(model, id) !== undefined);
  if (present.some((id) => isPartial(model, present, id))) {
    return { items: present, partial: true };
  }
  const out: string[] = [];
  const groups = new Set<string>();
  for (const id of present) {
    const group = itemOf(model, id)?.linkedGroup;
    if (group !== undefined && groupOf(model, id).length > 1) {
      if (groups.has(group)) continue;
      groups.add(group);
    }
    out.push(id);
  }
  return { items: out, partial: false };
}
