// SPDX-License-Identifier: GPL-3.0-or-later
// The sequence tabs above the timeline, as data (WP-2B.03): which sequences have a tab and in
// what order. Pure functions of the project's sequences (ID and name) and the saved
// `SequenceTabs` (the person's order and the tabs they closed). A sequence in neither list
// follows the ordered ones by ID, so a new sequence gets a tab at the end; closing a tab hides
// it and deletes nothing.
import type { Project, SequenceId } from "@cutweave/protocol";
import type { SequenceTabs } from "../state/viewStore.ts";

export interface SequenceTab {
  readonly id: SequenceId;
  readonly name: string;
}

const FIELD = "\u0000";
const ROW = "\u0001";

/**
 * The project's sequences as one string (IDs and names, by ID): a stable store selector. A
 * document patch that leaves the sequence list alone yields the same string, so what subscribes
 * with it does not render again.
 */
export function sequenceListKey(project: Project | null): string {
  if (project === null) return "";
  return Object.keys(project.sequences)
    .sort()
    .map((id) => `${id}${FIELD}${project.sequences[id as SequenceId]?.name ?? ""}`)
    .join(ROW);
}

/** The sequences a `sequenceListKey` names, by ID. */
export function sequencesOfKey(key: string): readonly SequenceTab[] {
  if (key === "") return [];
  return key.split(ROW).map((row) => {
    const at = row.indexOf(FIELD);
    return { id: row.slice(0, at) as SequenceId, name: row.slice(at + 1) };
  });
}

/** Every sequence in tab order: the saved order first, then the rest by ID. */
export function orderedSequences(
  all: readonly SequenceTab[],
  tabs: SequenceTabs,
): readonly SequenceTab[] {
  const byId = new Map(all.map((s) => [s.id, s]));
  const out: SequenceTab[] = [];
  for (const id of tabs.order) {
    const found = byId.get(id);
    if (found !== undefined && byId.delete(id)) out.push(found);
  }
  return [...out, ...byId.values()];
}

/** The tabs that show: every sequence not closed, and the active one even if it was. */
export function openTabs(
  all: readonly SequenceTab[],
  tabs: SequenceTabs,
  active: SequenceId | null,
): readonly SequenceTab[] {
  const shut = new Set(tabs.closed);
  const open = orderedSequences(all, tabs).filter((s) => s.id === active || !shut.has(s.id));
  // Never an empty strip while the project has a sequence.
  return open.length > 0 ? open : orderedSequences(all, tabs).slice(0, 1);
}

/** The sequences without a tab, in tab order (the "+" menu reopens them). */
export function closedSequences(
  all: readonly SequenceTab[],
  tabs: SequenceTabs,
  active: SequenceId | null,
): readonly SequenceTab[] {
  const open = new Set(openTabs(all, tabs, active).map((s) => s.id));
  return orderedSequences(all, tabs).filter((s) => !open.has(s.id));
}

const existing = (all: readonly SequenceTab[], ids: readonly SequenceId[]): SequenceId[] => {
  const known = new Set(all.map((s) => s.id));
  return ids.filter((id) => known.has(id));
};

/** Gives `id` a tab again. */
export function reopened(tabs: SequenceTabs, id: SequenceId): SequenceTabs {
  return tabs.closed.includes(id)
    ? { order: tabs.order, closed: tabs.closed.filter((c) => c !== id) }
    : tabs;
}

export interface Closed {
  readonly tabs: SequenceTabs;
  /** The sequence to show now, when the closed tab was the active one. */
  readonly show: SequenceId | null;
}

/**
 * Closes the tab of `id`. The last open tab stays (`null`). Closing the active tab shows its
 * right neighbour, or the left one at the end of the strip.
 */
export function closed(
  all: readonly SequenceTab[],
  tabs: SequenceTabs,
  active: SequenceId | null,
  id: SequenceId,
): Closed | null {
  const open = openTabs(all, tabs, active);
  const at = open.findIndex((s) => s.id === id);
  if (at < 0 || open.length < 2) return null;
  const next = open[at + 1] ?? open[at - 1];
  return {
    tabs: { order: existing(all, tabs.order), closed: [...existing(all, tabs.closed), id] },
    show: id === active && next !== undefined ? next.id : null,
  };
}

/** Closes every tab but the one of `id`. */
export function closedOthers(
  all: readonly SequenceTab[],
  tabs: SequenceTabs,
  id: SequenceId,
): SequenceTabs {
  return {
    order: existing(all, tabs.order),
    closed: all.map((s) => s.id).filter((other) => other !== id),
  };
}

/** Moves the tab of `id` to where the tab of `before` is (`null`: the end of the strip). */
export function moved(
  all: readonly SequenceTab[],
  tabs: SequenceTabs,
  id: SequenceId,
  before: SequenceId | null,
): SequenceTabs {
  if (id === before) return tabs;
  const order = orderedSequences(all, tabs)
    .map((s) => s.id)
    .filter((other) => other !== id);
  const at = before === null ? -1 : order.indexOf(before);
  order.splice(at < 0 ? order.length : at, 0, id);
  return { order, closed: existing(all, tabs.closed) };
}

/** Moves the tab of `id` one place left (`-1`) or right (`1`) among the open tabs. */
export function nudged(
  all: readonly SequenceTab[],
  tabs: SequenceTabs,
  active: SequenceId | null,
  id: SequenceId,
  by: -1 | 1,
): SequenceTabs {
  const open = openTabs(all, tabs, active);
  const at = open.findIndex((s) => s.id === id);
  const swap = open[at + by];
  if (at < 0 || swap === undefined) return tabs;
  // Left: before the neighbour. Right: before what follows the neighbour (the end, if nothing).
  return moved(all, tabs, id, by === -1 ? swap.id : (open[at + 2]?.id ?? null));
}

/** Puts the tab of `id` right after the tab of `after` (a copy sits next to its original). */
export function placedAfter(
  all: readonly SequenceTab[],
  tabs: SequenceTabs,
  id: SequenceId,
  after: SequenceId,
): SequenceTabs {
  const order = orderedSequences(all, tabs)
    .map((s) => s.id)
    .filter((other) => other !== id);
  const at = order.indexOf(after);
  order.splice(at < 0 ? order.length : at + 1, 0, id);
  return { order, closed: existing(all, tabs.closed).filter((c) => c !== id) };
}

/** The open tab `by` places from the active one, wrapping around (Next and Previous sequence). */
export function neighbour(
  all: readonly SequenceTab[],
  tabs: SequenceTabs,
  active: SequenceId | null,
  by: -1 | 1,
): SequenceId | null {
  const open = openTabs(all, tabs, active);
  if (open.length < 2) return null;
  const at = Math.max(
    0,
    open.findIndex((s) => s.id === active),
  );
  return open[(at + by + open.length) % open.length]?.id ?? null;
}

/** "Sequence 2", "Sequence 3"...: the first such name no sequence has. */
export function freshName(all: readonly SequenceTab[], name: (n: number) => string): string {
  const taken = new Set(all.map((s) => s.name));
  for (let n = all.length + 1; ; n++) {
    const candidate = name(n);
    if (!taken.has(candidate)) return candidate;
  }
}
