// SPDX-License-Identifier: GPL-3.0-or-later
// Linked groups in a selection (WP-2A.10, timeline spec INV-341): a click on the timeline selects
// a clip with its linked partners (its sound), so a selection of N clips holds more ids than
// clips. What acts on "the clips" (the clip menu's rows and names) counts one per group: the
// member selected first, which is the one clicked. A group only partly selected keeps each of its
// selected members.
import type { Project } from "@cutweave/protocol";

/** Item id to linked group, for the items of `project` that have one. */
function groupsOf(project: Project | null): Map<string, string> {
  const out = new Map<string, string>();
  for (const sequence of Object.values(project?.sequences ?? {})) {
    for (const track of Object.values(sequence.tracks)) {
      for (const item of Object.values(track.items)) {
        if (item.linked_group !== null) out.set(item.id, item.linked_group);
      }
    }
  }
  return out;
}

/** `ids` with every complete linked group reduced to its first member in `ids`. */
export function onePerGroup(project: Project | null, ids: readonly string[]): string[] {
  if (ids.length < 2) return [...ids];
  const groups = groupsOf(project);
  const selected = new Set(ids);
  const whole = new Set<string>();
  const partial = new Set<string>();
  for (const [id, group] of groups) {
    if (selected.has(id)) whole.add(group);
    else partial.add(group);
  }
  const seen = new Set<string>();
  return ids.filter((id) => {
    const group = groups.get(id);
    if (group === undefined || partial.has(group) || !whole.has(group)) return true;
    if (seen.has(group)) return false;
    seen.add(group);
    return true;
  });
}

/** `ids` with their linked partners after them (what a click with linked selection selects). */
export function withPartners(project: Project | null, ids: readonly string[]): string[] {
  const groups = groupsOf(project);
  const wanted = new Set(ids.map((id) => groups.get(id)).filter((g) => g !== undefined));
  const out = [...ids];
  for (const [id, group] of groups) {
    if (wanted.has(group) && !out.includes(id)) out.push(id);
  }
  return out;
}
