// SPDX-License-Identifier: GPL-3.0-or-later
// The sequence actions (WP-2B.03): each sends one catalog command, shows what it made, and keeps
// the tabs in step. The engine here is a mock transport that records what was asked.
import {
  MockTransport,
  type Project,
  RpcClient,
  RpcError,
  type SequenceId,
} from "@cutweave/protocol";
import { afterEach, describe, expect, it } from "vitest";
import { createEngineStore } from "../engine/engineStore.ts";
import { createDocStore } from "../state/docStore.ts";
import { setChosenSequence } from "../state/selectors.ts";
import { bindShownSequence } from "../state/shownSequence.ts";
import { createToastStore } from "../state/toastStore.ts";
import { createViewStore } from "../state/viewStore.ts";
import { sampleProject } from "../testing/fixtures.ts";
import {
  closeOtherTabs,
  closeTab,
  createSequence,
  deleteSequence,
  duplicateSequence,
  nudgeTab,
  renameSequence,
  type SequenceDeps,
  setSequenceAspect,
  showNeighbour,
  shownSequence,
  showSequence,
} from "./actions.ts";

const A = "seq_1" as SequenceId;
const B = "seq_2" as SequenceId;
const C = "seq_3" as SequenceId;

/** The fixture's project with `ids` as extra, empty sequences named after their ID. */
function projectWith(...ids: SequenceId[]): Project {
  const project = sampleProject();
  const first = project.sequences[A];
  if (first === undefined) throw new Error("the fixture has no seq_1");
  const more = Object.fromEntries(
    ids.map((id) => [id, { ...first, id, name: id, track_order: [], tracks: {} }]),
  );
  return { ...project, sequences: { ...project.sequences, ...more } } as Project;
}

function setup(project: Project = projectWith(B, C), magnetic = true) {
  const transport = new MockTransport();
  const engine = createEngineStore();
  engine.setState({ rpc: new RpcClient(transport) });
  const doc = createDocStore();
  doc.setState({ project });
  const view = createViewStore();
  const toasts = createToastStore();
  const off = bindShownSequence(view, doc);
  const deps: SequenceDeps = {
    engine,
    doc,
    view,
    toasts,
    t: (key, vars) => (vars === undefined ? key : `${key} ${JSON.stringify(vars)}`),
    magneticMain: () => magnetic,
  };
  const sent = () => transport.requests.map((r) => [r.method, r.params]);
  return { transport, doc, view, toasts, deps, off, sent };
}

afterEach(() => setChosenSequence(null));

describe("createSequence", () => {
  it("sends sequence.create with a free name and the magnetic preference, then shows it", async () => {
    const s = setup(projectWith(), true);
    s.transport.handle("sequence.create", () => {
      // The engine's patch lands before its answer.
      s.doc.setState({ project: projectWith(B) });
      return { sequence: B, video_track: "trk_v2", audio_track: "trk_a2", format: {} };
    });
    expect(await createSequence("9:16", s.deps)).toBe(B);
    expect(s.sent()).toEqual([
      [
        "sequence.create",
        { name: 'sequences.newName {"n":"2"}', preset: "9:16", main_magnetic: true },
      ],
    ]);
    expect(s.view.getState().activeSequence).toBe(B);
    expect(shownSequence(s.deps)).toBe(B);
    s.off();
  });

  it("without a preset the engine uses the project's format; the preference off sends false", async () => {
    const s = setup(projectWith(), false);
    s.transport.handle("sequence.create", () => ({ sequence: B }));
    await createSequence(null, s.deps);
    expect(s.sent()[0]?.[1]).toEqual({ name: 'sequences.newName {"n":"2"}', main_magnetic: false });
    s.off();
  });

  it("a refusal is a toast with the engine's words and nothing else changes", async () => {
    const s = setup();
    s.transport.handle("sequence.create", () => {
      throw new RpcError("invalid", "the name is too long");
    });
    expect(await createSequence(null, s.deps)).toBeNull();
    expect(s.toasts.getState().notices.at(-1)).toMatchObject({
      tone: "danger",
      title: "sequences.failed.create",
      message: "the name is too long",
    });
    expect(s.view.getState().activeSequence).toBeNull();
    s.off();
  });
});

describe("duplicate, rename, delete, aspect", () => {
  it("a copy gets its tab right after the original and shows", async () => {
    const s = setup(projectWith(B));
    s.transport.handle("sequence.duplicate", () => {
      s.doc.setState({ project: projectWith(B, C) });
      return { sequence: C, name: "Main copy", item_count: 2 };
    });
    expect(await duplicateSequence(A, s.deps)).toBe(C);
    expect(s.sent()).toEqual([["sequence.duplicate", { sequence: A }]]);
    expect(s.view.getState().sequenceTabs.order).toEqual([A, C, B]);
    expect(shownSequence(s.deps)).toBe(C);
    s.off();
  });

  it("rename sends the trimmed name; blank and unchanged names send nothing", async () => {
    const s = setup();
    s.transport.handle("sequence.rename", () => ({}));
    expect(await renameSequence(A, "   ", s.deps)).toBe(false);
    expect(await renameSequence(A, "Main", s.deps)).toBe(false);
    expect(await renameSequence(A, "  Cut 2 ", s.deps)).toBe(true);
    expect(s.sent()).toEqual([["sequence.rename", { sequence: A, name: "Cut 2" }]]);
    s.off();
  });

  it("deleting the shown sequence shows its neighbour", async () => {
    const s = setup();
    showSequence(B, s.deps);
    s.transport.handle("sequence.delete", () => {
      s.doc.setState({ project: projectWith(C) });
      return {};
    });
    expect(await deleteSequence(B, s.deps)).toBe(true);
    expect(s.sent()).toEqual([["sequence.delete", { sequence: B }]]);
    expect(shownSequence(s.deps)).toBe(C);
    s.off();
  });

  it("an aspect preset is one sequence.set_format with fit; the same shape sends nothing", async () => {
    const s = setup();
    s.transport.handle("sequence.set_format", () => ({}));
    // The fixture's sequence is 1920 x 1080 already.
    expect(await setSequenceAspect(A, "16:9", s.deps)).toBe(false);
    expect(await setSequenceAspect(A, "9:16", s.deps)).toBe(true);
    expect(s.sent()).toEqual([
      ["sequence.set_format", { sequence: A, width: 1080, height: 1920, reframe: "fit" }],
    ]);
    s.off();
  });
});

describe("the tabs", () => {
  it("showing, closing and reopening never call a document command", () => {
    const s = setup();
    showSequence(B, s.deps);
    expect(closeTab(B, s.deps)).toBe(true);
    // Its right neighbour shows; B has no tab.
    expect(shownSequence(s.deps)).toBe(C);
    expect(s.view.getState().sequenceTabs.closed).toEqual([B]);
    showSequence(B, s.deps);
    expect(s.view.getState().sequenceTabs.closed).toEqual([]);
    closeOtherTabs(A, s.deps);
    expect(shownSequence(s.deps)).toBe(A);
    expect(s.view.getState().sequenceTabs.closed).toEqual([B, C]);
    // The last tab stays.
    expect(closeTab(A, s.deps)).toBe(false);
    expect(s.sent()).toEqual([]);
    s.off();
  });

  it("next and previous wrap; a tab moves one place", () => {
    const s = setup();
    showNeighbour(-1, s.deps);
    expect(shownSequence(s.deps)).toBe(C);
    showNeighbour(1, s.deps);
    expect(shownSequence(s.deps)).toBe(A);
    nudgeTab(A, 1, s.deps);
    expect(s.view.getState().sequenceTabs.order).toEqual([B, A, C]);
    s.off();
  });
});
