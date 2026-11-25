// SPDX-License-Identifier: GPL-3.0-or-later
// The History panel's rows and jumps (WP-2H.11): which entries are rows, where the marker sits,
// how many `*` steps a click on a row takes.
import type { CheckpointId } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { entry, mixedHistory } from "../testing/historyFixtures.ts";
import {
  actorKind,
  buildRows,
  chunkSteps,
  markerIndex,
  matchesFilter,
  planJump,
  relativeTime,
} from "./historyModel.ts";

describe("rows", () => {
  it("lists changes oldest first, with no undo records and no change that cannot be redone", () => {
    const rows = buildRows(mixedHistory());
    expect(rows.map((r) => r.txId)).toEqual([
      "tx_2",
      "tx_3",
      "tx_4",
      "tx_5",
      "tx_6",
      "tx_7",
      "tx_8",
    ]);
    expect(rows.map((r) => r.state)).toEqual([
      "applied",
      "applied",
      "applied",
      "applied",
      "applied",
      "undone",
      "undone",
    ]);
  });

  it("names who made each step and puts the marker under the current state", () => {
    const rows = buildRows(mixedHistory());
    expect(rows.map((r) => r.kind)).toEqual([
      "user",
      "user",
      "user",
      "agent",
      "plugin",
      "user",
      "agent",
    ]);
    expect(markerIndex(rows)).toBe(5);
    expect(markerIndex([])).toBe(0);
  });

  it("keeps a checkpoint restore as a row of its own", () => {
    const rows = buildRows([
      entry(2, "edit.restore", "user", {
        cause: { type: "restore", checkpoint: "cp_1" as CheckpointId },
      }),
      entry(1, "timeline.insert", "user"),
    ]);
    expect(rows.map((r) => [r.txId, r.restore])).toEqual([
      ["tx_1", false],
      ["tx_2", true],
    ]);
  });

  it("leaves out an undone change that sits before an applied one", () => {
    // The agent's trim was undone, then the person kept editing: the list stays linear.
    const rows = buildRows([
      entry(3, "Move clip", "user"),
      entry(2, "Trim clip", "agent:claude", { status: "undone" }),
      entry(1, "timeline.insert", "user"),
    ]);
    expect(rows.map((r) => r.txId)).toEqual(["tx_1", "tx_3"]);
    expect(markerIndex(rows)).toBe(2);
  });

  it("puts the marker before everything when every change is undone", () => {
    const rows = buildRows([
      entry(2, "b", "user", { status: "undone" }),
      entry(1, "a", "user", { status: "undone" }),
    ]);
    expect(markerIndex(rows)).toBe(0);
  });

  it("classifies actors and the filter", () => {
    expect(["user", "agent:cli", "plugin:x.y", "system"].map(actorKind)).toEqual([
      "user",
      "agent",
      "plugin",
      "system",
    ]);
    const [first, , , agent] = buildRows(mixedHistory());
    expect(first && matchesFilter(first, "user")).toBe(true);
    expect(first && matchesFilter(first, "agent")).toBe(false);
    expect(agent && matchesFilter(agent, "agent")).toBe(true);
    expect(agent && matchesFilter(agent, "user")).toBe(false);
    expect(agent && matchesFilter(agent, "all")).toBe(true);
  });
});

describe("jumps", () => {
  const rows = buildRows(mixedHistory());

  it("goes back by undoing every change after the row, newest first", () => {
    expect(planJump(rows, "tx_4")).toEqual({ kind: "undo", steps: 2 });
    expect(planJump(rows, "tx_2")).toEqual({ kind: "undo", steps: 4 });
  });

  it("does nothing on the row the document is already at", () => {
    expect(planJump(rows, "tx_6")).toEqual({ kind: "none" });
    expect(planJump(rows, "tx_missing")).toEqual({ kind: "none" });
  });

  it("goes forward from a dimmed row by redoing up to it", () => {
    expect(planJump(rows, "tx_7")).toEqual({ kind: "redo", steps: 1 });
    expect(planJump(rows, "tx_8")).toEqual({ kind: "redo", steps: 2 });
  });

  it("does not count a change that cannot be undone", () => {
    const withStamp = buildRows([
      entry(3, "timeline.insert", "user"),
      entry(2, "project.stamp", "system", { can_undo: false }),
      entry(1, "media.register", "user"),
    ]);
    expect(planJump(withStamp, "tx_1")).toEqual({ kind: "undo", steps: 1 });
  });

  it("splits a long jump into calls of at most 100 steps", () => {
    expect(chunkSteps(5)).toEqual([5]);
    expect(chunkSteps(100)).toEqual([100]);
    expect(chunkSteps(250)).toEqual([100, 100, 50]);
  });
});

describe("time", () => {
  it("says how long ago a step was made", () => {
    const at = 1_000_000_000_000;
    expect(relativeTime(at, at + 5_000)).toEqual({ unit: "now" });
    expect(relativeTime(at, at + 120_000)).toEqual({ unit: "minute", count: 2 });
    expect(relativeTime(at, at + 3 * 3_600_000)).toEqual({ unit: "hour", count: 3 });
    expect(relativeTime(at, at + 2 * 86_400_000)).toEqual({ unit: "day", count: 2 });
    expect(relativeTime(at, at - 5_000)).toEqual({ unit: "now" });
  });
});
