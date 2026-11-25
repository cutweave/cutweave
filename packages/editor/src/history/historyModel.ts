// SPDX-License-Identifier: GPL-3.0-or-later
// The History panel's rows and the jumps between them (WP-2H.11, ADR-0019). Pure: the engine's
// `edit.history` entries in, rows and `edit.undo` / `edit.redo` step counts out.
//
// The engine lists every committed transaction, newest first, including the undo and redo
// records. A person's history is the changes (cause `edit` or `restore`): the undo and redo
// records only move the marker, and a change that was undone and can no longer be redone (a new
// edit cleared it) is gone.

import type { HistoryEntry } from "@cutweave/protocol";

/** Who made a step, for the filter and the colour. */
export type ActorKind = "user" | "agent" | "plugin" | "system";

export type HistoryFilter = "all" | "user" | "agent";

/** One change in the list. */
export interface HistoryRow {
  readonly txId: string;
  /** The transaction label or command name as the engine logged it (see `historyLabel`). */
  readonly label: string;
  /** The actor id: `user`, `agent:claude`, `plugin:acme.luts`, `system`. */
  readonly actor: string;
  readonly kind: ActorKind;
  /** When it committed, in milliseconds since the Unix epoch. */
  readonly at: number;
  /** Its changes are applied now (above the marker) or undone and redoable (below it). */
  readonly state: "applied" | "undone";
  /** `edit.undo{*}` can revert it (it is on its actor's undo stack). */
  readonly undoable: boolean;
  /** A checkpoint restore, not an ordinary edit. */
  readonly restore: boolean;
}

export function actorKind(actor: string): ActorKind {
  if (actor === "user") return "user";
  if (actor === "system") return "system";
  if (actor.startsWith("agent:") || actor === "agent") return "agent";
  if (actor.startsWith("plugin:")) return "plugin";
  return "system";
}

/**
 * The rows, oldest first, from the engine's page of entries (newest first). Undo and redo
 * records are not rows, and nor is a change that was undone and cannot be redone, or that was
 * undone before a change that is applied now.
 */
export function buildRows(entries: readonly HistoryEntry[]): HistoryRow[] {
  const rows: HistoryRow[] = [];
  for (let i = entries.length - 1; i >= 0; i -= 1) {
    const entry = entries[i] as HistoryEntry;
    if (entry.cause.type !== "edit" && entry.cause.type !== "restore") continue;
    if (entry.status === "undone" && !entry.can_redo) continue;
    rows.push({
      txId: entry.tx_id,
      label: entry.label,
      actor: entry.actor,
      kind: actorKind(entry.actor),
      at: entry.at,
      state: entry.status === "undone" ? "undone" : "applied",
      undoable: entry.can_undo,
      restore: entry.cause.type === "restore",
    });
  }
  // The list is linear: what can be redone sits after the current state. An undone change
  // before a later applied one (another actor's, undone while the person kept editing) is not
  // shown; Redo still reaches it, and it is a row again once it is applied.
  const lastApplied = rows.reduce((at, row, i) => (row.state === "applied" ? i : at), -1);
  return rows.filter((row, i) => row.state === "applied" || i > lastApplied);
}

/**
 * Where the marker sits: after the last applied row. Rows after it are the undone tail a redo
 * would bring back.
 */
export function markerIndex(rows: readonly HistoryRow[]): number {
  for (let i = rows.length - 1; i >= 0; i -= 1) {
    if ((rows[i] as HistoryRow).state === "applied") return i + 1;
  }
  return 0;
}

/** Whether `filter` leaves a row undimmed. */
export function matchesFilter(row: HistoryRow, filter: HistoryFilter): boolean {
  if (filter === "all") return true;
  return row.kind === filter;
}

/** What it takes to get back to a step: that many `*` undos or redos, none when already there. */
export type JumpPlan =
  | { readonly kind: "none" }
  | { readonly kind: "undo"; readonly steps: number }
  | { readonly kind: "redo"; readonly steps: number };

/** The most steps one `edit.undo` / `edit.redo` takes (the engine's limit). */
export const MAX_STEPS = 100;

/**
 * The jump to the state right after row `txId`. A row above the marker: every undoable change
 * after it is undone, newest first. A dimmed row: the undone changes up to and including it are
 * redone, oldest first, which is the order `edit.redo{*}` takes them in.
 */
export function planJump(rows: readonly HistoryRow[], txId: string): JumpPlan {
  const at = rows.findIndex((r) => r.txId === txId);
  if (at < 0) return { kind: "none" };
  const target = rows[at] as HistoryRow;
  if (target.state === "applied") {
    let steps = 0;
    for (let i = at + 1; i < rows.length; i += 1) {
      const row = rows[i] as HistoryRow;
      if (row.state === "applied" && row.undoable) steps += 1;
    }
    return steps === 0 ? { kind: "none" } : { kind: "undo", steps };
  }
  let steps = 0;
  for (let i = 0; i <= at; i += 1) {
    if ((rows[i] as HistoryRow).state === "undone") steps += 1;
  }
  return { kind: "redo", steps };
}

/** Splits a plan into engine calls of at most {@link MAX_STEPS} each. */
export function chunkSteps(steps: number): number[] {
  const out: number[] = [];
  for (let left = steps; left > 0; left -= MAX_STEPS) out.push(Math.min(left, MAX_STEPS));
  return out;
}

// ---- time ---------------------------------------------------------------------------------------

export type RelativeTime =
  | { readonly unit: "now" }
  | { readonly unit: "minute" | "hour" | "day"; readonly count: number };

/** "just now", "2 minutes", "3 hours", "5 days": how long before `now` a step committed. */
export function relativeTime(at: number, now: number): RelativeTime {
  const seconds = Math.max(0, Math.floor((now - at) / 1000));
  if (seconds < 60) return { unit: "now" };
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return { unit: "minute", count: minutes };
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return { unit: "hour", count: hours };
  return { unit: "day", count: Math.floor(hours / 24) };
}
