// SPDX-License-Identifier: GPL-3.0-or-later
// History entries for tests: what `edit.history` lists, newest first.
import type { HistoryEntry, TxId } from "@cutweave/protocol";

type Over = Partial<Omit<HistoryEntry, "tx_id" | "label" | "actor">>;

/** One change (or, with `over.cause`, an undo, redo or restore record). */
export function entry(tx: number, label: string, actor: string, over: Over = {}): HistoryEntry {
  const status = over.status ?? "applied";
  return {
    tx_id: `tx_${tx}`,
    actor,
    label,
    kind: null,
    cause: { type: "edit" },
    prev_rev: tx - 1,
    rev: tx,
    at: 1_700_000_000_000 + tx * 1000,
    status,
    can_undo: status === "applied",
    can_redo: status === "undone",
    ops: 1,
    ...over,
  } as HistoryEntry;
}

/**
 * A mixed history, newest first: your import, insert and move, an agent's trim, a plugin's
 * change, then an undone move of yours and an undone trim of the agent's (the redoable tail), an
 * undo record, and an old undone change that a later edit made unredoable.
 */
export function mixedHistory(): HistoryEntry[] {
  const e = entry;
  return [
    e(9, "undo", "user", { cause: { type: "undo", of: "tx_8" as TxId } }),
    e(8, "Trim clip", "agent:claude", { status: "undone" }),
    e(7, "Move clip", "user", { status: "undone" }),
    e(6, "item.set_props", "plugin:acme.luts"),
    e(5, "clip.trim", "agent:claude"),
    e(4, "Move clip", "user"),
    e(3, "timeline.insert", "user"),
    e(2, "media.register", "user"),
    e(1, "Rename clip", "user", { status: "undone", can_redo: false }),
  ];
}
