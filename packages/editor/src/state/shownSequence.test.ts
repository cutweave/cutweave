// SPDX-License-Identifier: GPL-3.0-or-later
import type { Project, SequenceId } from "@cutweave/protocol";
import { afterEach, describe, expect, it } from "vitest";
import { sampleProject } from "../testing/fixtures.ts";
import { createDocStore } from "./docStore.ts";
import { selectActiveSequenceId, selectTimelineModel, setChosenSequence } from "./selectors.ts";
import { bindShownSequence } from "./shownSequence.ts";
import { createViewStore } from "./viewStore.ts";

const A = "seq_1" as SequenceId;
const B = "seq_2" as SequenceId;

/** The fixture's project with a second, empty 9:16 sequence. */
function twoSequences(): Project {
  const project = sampleProject();
  const first = project.sequences[A];
  if (first === undefined) throw new Error("the fixture has no seq_1");
  return {
    ...project,
    sequences: {
      ...project.sequences,
      [B]: {
        ...first,
        id: B,
        name: "Vertical",
        format: { ...first.format, width: 1080, height: 1920 },
        track_order: [],
        tracks: {},
      },
    },
  } as Project;
}

function setup() {
  const view = createViewStore();
  const doc = createDocStore();
  doc.setState({ project: twoSequences() });
  const off = bindShownSequence(view, doc);
  return { view, doc, off };
}

afterEach(() => setChosenSequence(null));

describe("bindShownSequence", () => {
  it("shows the first sequence until one is chosen, then the chosen one", () => {
    const { view, doc, off } = setup();
    expect(selectActiveSequenceId(doc.getState().project)).toBe(A);
    view.getState().setActiveSequence(B);
    expect(selectActiveSequenceId(doc.getState().project)).toBe(B);
    expect(selectTimelineModel(doc.getState().project).items).toHaveLength(0);
    off();
    expect(selectActiveSequenceId(doc.getState().project)).toBe(A);
  });

  it("publishes the document again on a switch, and only then", () => {
    const { view, doc, off } = setup();
    let published = 0;
    doc.subscribe(() => published++);
    const before = doc.getState().project;
    // View changes that are not a switch leave the document alone.
    view.getState().setPlayhead(1000);
    view.getState().setSelection([{ kind: "item", id: "itm_v" as never }]);
    expect(published).toBe(0);
    view.getState().setActiveSequence(B);
    expect(published).toBe(1);
    // The same contents under a new object: what was derived from the project is derived again.
    expect(doc.getState().project).not.toBe(before);
    expect(doc.getState().project?.sequences).toBe(before?.sequences);
    // Choosing it again, or choosing the one that already shows, publishes nothing.
    view.getState().setActiveSequence(B);
    expect(published).toBe(1);
    off();
  });

  it("a switch empties the selection: it was of the sequence before", () => {
    const { view, off } = setup();
    view.getState().setSelection([{ kind: "item", id: "itm_v" as never }]);
    view.getState().setActiveSequence(B);
    expect(view.getState().selection).toEqual([]);
    off();
  });

  it("a chosen sequence the mirror does not hold shows once it arrives; a deleted one falls back", () => {
    const view = createViewStore();
    const doc = createDocStore();
    const both = twoSequences();
    const { [B]: _later, ...onlyA } = both.sequences;
    doc.setState({ project: { ...both, sequences: onlyA } as Project });
    const off = bindShownSequence(view, doc);
    view.getState().setActiveSequence(B);
    expect(selectActiveSequenceId(doc.getState().project)).toBe(A);
    doc.setState({ project: both });
    expect(selectActiveSequenceId(doc.getState().project)).toBe(B);
    doc.setState({ project: { ...both, sequences: onlyA } as Project });
    expect(selectActiveSequenceId(doc.getState().project)).toBe(A);
    off();
  });
});
