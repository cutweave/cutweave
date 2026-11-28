// SPDX-License-Identifier: GPL-3.0-or-later
import type { Project, SequenceId } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { NO_SEQUENCE_TABS } from "../state/viewStore.ts";
import { sampleProject } from "../testing/fixtures.ts";
import {
  closed,
  closedOthers,
  closedSequences,
  freshName,
  moved,
  neighbour,
  nudged,
  openTabs,
  placedAfter,
  reopened,
  type SequenceTab,
  sequenceListKey,
  sequencesOfKey,
} from "./tabs.ts";

const id = (s: string) => s as SequenceId;
const A = id("seq_a");
const B = id("seq_b");
const C = id("seq_c");
const ALL: readonly SequenceTab[] = [
  { id: A, name: "Main" },
  { id: B, name: "Vertical" },
  { id: C, name: "Square" },
];
const ids = (tabs: readonly SequenceTab[]) => tabs.map((t) => t.id);

describe("sequenceListKey", () => {
  it("names the sequences by ID and survives a round trip", () => {
    const project = sampleProject();
    expect(sequencesOfKey(sequenceListKey(project))).toEqual([{ id: "seq_1", name: "Main" }]);
    expect(sequencesOfKey(sequenceListKey(null))).toEqual([]);
  });

  it("is the same string after a patch that leaves the sequence list alone", () => {
    const project = sampleProject();
    const seq = project.sequences["seq_1" as SequenceId];
    const edited = {
      ...project,
      sequences: { seq_1: { ...seq, in_out: { start: 0, end: 10 } } },
    } as unknown as Project;
    expect(sequenceListKey(edited)).toBe(sequenceListKey(project));
    const renamed = {
      ...project,
      sequences: { seq_1: { ...seq, name: "Cut 2" } },
    } as unknown as Project;
    expect(sequenceListKey(renamed)).not.toBe(sequenceListKey(project));
  });
});

describe("the tabs", () => {
  it("every sequence has a tab, by ID, until the person orders or closes one", () => {
    expect(ids(openTabs(ALL, NO_SEQUENCE_TABS, A))).toEqual([A, B, C]);
    expect(closedSequences(ALL, NO_SEQUENCE_TABS, A)).toEqual([]);
  });

  it("the saved order comes first; new sequences follow; gone ones are skipped", () => {
    const tabs = { order: [C, id("seq_gone"), A], closed: [] };
    expect(ids(openTabs(ALL, tabs, A))).toEqual([C, A, B]);
  });

  it("closing hides a tab; the active sequence always has one; the last tab stays", () => {
    const one = closed(ALL, NO_SEQUENCE_TABS, A, B);
    expect(one).toEqual({ tabs: { order: [], closed: [B] }, show: null });
    if (one === null) throw new Error("not closed");
    expect(ids(openTabs(ALL, one.tabs, A))).toEqual([A, C]);
    expect(ids(closedSequences(ALL, one.tabs, A))).toEqual([B]);
    // Shown by an agent or the palette while closed: it has a tab again.
    expect(ids(openTabs(ALL, one.tabs, B))).toEqual([A, B, C]);
    const two = closed(ALL, one.tabs, A, C);
    if (two === null) throw new Error("not closed");
    expect(closed(ALL, two.tabs, A, A)).toBeNull();
  });

  it("closing the active tab shows its right neighbour, or the left one at the end", () => {
    expect(closed(ALL, NO_SEQUENCE_TABS, B, B)?.show).toBe(C);
    expect(closed(ALL, NO_SEQUENCE_TABS, C, C)?.show).toBe(B);
  });

  it("close others, reopen", () => {
    const tabs = closedOthers(ALL, NO_SEQUENCE_TABS, B);
    expect(ids(openTabs(ALL, tabs, B))).toEqual([B]);
    expect(ids(openTabs(ALL, reopened(tabs, C), B))).toEqual([B, C]);
    expect(reopened(NO_SEQUENCE_TABS, C)).toBe(NO_SEQUENCE_TABS);
  });

  it("a tab moves before another, to the end, and one place at a time", () => {
    expect(ids(openTabs(ALL, moved(ALL, NO_SEQUENCE_TABS, C, A), A))).toEqual([C, A, B]);
    expect(ids(openTabs(ALL, moved(ALL, NO_SEQUENCE_TABS, A, null), A))).toEqual([B, C, A]);
    expect(moved(ALL, NO_SEQUENCE_TABS, A, A)).toBe(NO_SEQUENCE_TABS);
    expect(ids(openTabs(ALL, nudged(ALL, NO_SEQUENCE_TABS, A, B, -1), A))).toEqual([B, A, C]);
    expect(ids(openTabs(ALL, nudged(ALL, NO_SEQUENCE_TABS, A, B, 1), A))).toEqual([A, C, B]);
    // At the edge nothing changes.
    expect(nudged(ALL, NO_SEQUENCE_TABS, A, A, -1)).toBe(NO_SEQUENCE_TABS);
    expect(nudged(ALL, NO_SEQUENCE_TABS, A, C, 1)).toBe(NO_SEQUENCE_TABS);
    // A closed tab between two open ones is stepped over.
    const hidden = { order: [], closed: [B] };
    expect(ids(openTabs(ALL, nudged(ALL, hidden, A, C, -1), A))).toEqual([C, A]);
  });

  it("a copy sits right after its original and has a tab", () => {
    const tabs = placedAfter(ALL, { order: [], closed: [C] }, C, A);
    expect(ids(openTabs(ALL, tabs, A))).toEqual([A, C, B]);
  });

  it("next and previous wrap around the open tabs", () => {
    expect(neighbour(ALL, NO_SEQUENCE_TABS, C, 1)).toBe(A);
    expect(neighbour(ALL, NO_SEQUENCE_TABS, A, -1)).toBe(C);
    expect(neighbour(ALL, { order: [], closed: [B] }, A, 1)).toBe(C);
    expect(neighbour([{ id: A, name: "Main" }], NO_SEQUENCE_TABS, A, 1)).toBeNull();
  });

  it("a new sequence gets the first free numbered name", () => {
    const name = (n: number) => `Sequence ${n}`;
    expect(freshName([{ id: A, name: "Main" }], name)).toBe("Sequence 2");
    expect(freshName([...ALL, { id: id("seq_d"), name: "Sequence 5" }], name)).toBe("Sequence 6");
    const taken = [
      { id: A, name: "Main" },
      { id: B, name: "Sequence 3" },
    ];
    expect(freshName(taken, name)).toBe("Sequence 4");
  });
});
