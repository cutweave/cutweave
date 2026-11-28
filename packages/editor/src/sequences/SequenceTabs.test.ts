// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The sequence tabs (WP-2B.03): what a click, a double-click, the keys and the "+" menu do, and
// the performance rule of issue #241: the strip renders only when the list of sequences, the
// active sequence or the tab order changes. A hundred document patches of a drag (the clip's
// `start` changes on every update) and a moving playhead must not render it once.
import {
  type DocPatched,
  MockTransport,
  type PatchOp,
  type Project,
  RpcClient,
  type SequenceId,
} from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createElement as h, Profiler } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { docStore } from "../state/docStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { bindShownSequence } from "../state/shownSequence.ts";
import { initialViewState, NO_SEQUENCE_TABS, viewStore } from "../state/viewStore.ts";
import { sampleProject, sec } from "../testing/fixtures.ts";
import { SequenceTabs, tabStripRenders } from "./SequenceTabs.tsx";
import { sequenceUiStore } from "./uiStore.ts";

const A = "seq_1" as SequenceId;
const B = "seq_2" as SequenceId;
const C = "seq_3" as SequenceId;

function projectWith(names: Readonly<Record<string, string>>): Project {
  const project = sampleProject();
  const first = project.sequences[A];
  if (first === undefined) throw new Error("the fixture has no seq_1");
  const more = Object.fromEntries(
    Object.entries(names).map(([id, name]) => [
      id,
      { ...first, id, name, track_order: [], tracks: {} },
    ]),
  );
  return { ...project, sequences: { ...project.sequences, ...more } } as Project;
}

const load = (project: Project): void =>
  docStore.getState().resync({ rev: 1, committed_rev: 1, project });

/** A drag update: the engine moved a clip of the first sequence. */
const movePatch = (rev: number, old: number, start: number): DocPatched =>
  ({
    rev,
    prev_rev: rev - 1,
    committed_rev: 1,
    actor: "user",
    tx_id: "tx_drag",
    cause: "command",
    request_id: null,
    ops: [
      { op: "replace", path: "/sequences/seq_1/tracks/trk_v/items/itm_v/start", old, value: start },
    ] as unknown as PatchOp[],
  }) as unknown as DocPatched;

let transport: MockTransport;
let off: () => void;

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
});

beforeEach(() => {
  viewStore.setState({ ...initialViewState, layouts: {}, sequenceTabs: NO_SEQUENCE_TABS });
  sequenceUiStore.setState({ renaming: null, settings: null });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  transport = new MockTransport();
  engineStore.setState({ ...initialEngineState, rpc: new RpcClient(transport) });
  off = bindShownSequence(viewStore, docStore);
  load(projectWith({ [B]: "Vertical", [C]: "Square" }));
});

afterEach(() => {
  cleanup();
  off();
  engineStore.setState({ ...initialEngineState });
});

function mount() {
  const commits = { n: 0 };
  render(
    h(
      I18nextProvider,
      { i18n },
      h(
        TooltipProvider,
        null,
        h(
          Profiler,
          {
            id: "tabs",
            onRender: () => {
              commits.n += 1;
            },
          },
          h(SequenceTabs),
        ),
      ),
    ),
  );
  return commits;
}

const tabNames = () => screen.getAllByRole("tab").map((tab) => tab.textContent);
const selected = () =>
  screen
    .getAllByRole("tab")
    .filter((tab) => tab.getAttribute("aria-selected") === "true")
    .map((tab) => tab.textContent);
const shown = () => selectActiveSequenceId(docStore.getState().project);

describe("the tab strip renders only when the tabs change (#241)", () => {
  it("100 document patches that leave the sequence list alone cause no render", () => {
    const commits = mount();
    const before = { commits: commits.n, renders: tabStripRenders.count };
    let at = sec(1);
    for (let i = 0; i < 100; i++) {
      const next = at + sec(0.01);
      act(() => {
        expect(docStore.getState().applyEvent(movePatch(2 + i, at, next))).toBe("applied");
      });
      at = next;
    }
    expect(commits.n - before.commits).toBe(0);
    expect(tabStripRenders.count - before.renders).toBe(0);
  });

  it("a moving playhead, a selection, a zoom and a scroll cause no render", () => {
    const commits = mount();
    const before = { commits: commits.n, renders: tabStripRenders.count };
    act(() => {
      for (let i = 0; i < 100; i++) {
        viewStore.getState().setPlayhead(sec(i / 30));
        viewStore.getState().setScroll(i, 0);
      }
      viewStore.getState().setZoom(240);
      viewStore.getState().setSelection([{ kind: "item", id: "itm_v" as never }]);
    });
    expect(commits.n - before.commits).toBe(0);
    expect(tabStripRenders.count - before.renders).toBe(0);
  });

  it("a rename by anyone and a switch render it once each", () => {
    mount();
    const project = docStore.getState().project;
    const second = project?.sequences[B];
    if (project == null || second === undefined) throw new Error("no project");
    let before = tabStripRenders.count;
    act(() => {
      docStore.getState().resync({
        rev: 2,
        committed_rev: 2,
        project: {
          ...project,
          sequences: { ...project.sequences, [B]: { ...second, name: "Reel" } },
        },
      });
    });
    expect(tabStripRenders.count - before).toBe(1);
    expect(tabNames()).toEqual(["Main", "Reel", "Square"]);
    before = tabStripRenders.count;
    act(() => viewStore.getState().setActiveSequence(C));
    expect(tabStripRenders.count - before).toBe(1);
    expect(selected()).toEqual(["Square"]);
  });
});

describe("the tabs", () => {
  it("lists the sequences, the first one selected, and a click shows another", () => {
    mount();
    expect(screen.getByRole("tablist", { name: "Sequences" })).toBeTruthy();
    expect(tabNames()).toEqual(["Main", "Vertical", "Square"]);
    expect(selected()).toEqual(["Main"]);
    fireEvent.click(screen.getByRole("tab", { name: "Vertical" }));
    expect(shown()).toBe(B);
    expect(selected()).toEqual(["Vertical"]);
    expect(transport.requests).toEqual([]);
  });

  it("the arrow keys move along the tabs and wrap", () => {
    mount();
    fireEvent.keyDown(screen.getByRole("tab", { name: "Main" }), { key: "ArrowLeft" });
    expect(shown()).toBe(C);
    fireEvent.keyDown(screen.getByRole("tab", { name: "Square" }), { key: "ArrowRight" });
    expect(shown()).toBe(A);
    fireEvent.keyDown(screen.getByRole("tab", { name: "Main" }), { key: "End" });
    expect(shown()).toBe(C);
  });

  it("a double-click renames in place: Enter sends sequence.rename, Escape sends nothing", async () => {
    transport.handle("sequence.rename", () => ({}));
    mount();
    fireEvent.doubleClick(screen.getByRole("tab", { name: "Vertical" }));
    const field = screen.getByRole("textbox", { name: "Sequence name" }) as HTMLInputElement;
    expect(field.value).toBe("Vertical");
    fireEvent.keyDown(field, { key: "Escape" });
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(transport.requests).toEqual([]);
    fireEvent.keyDown(screen.getByRole("tab", { name: "Vertical" }), { key: "F2" });
    const again = screen.getByRole("textbox", { name: "Sequence name" }) as HTMLInputElement;
    fireEvent.change(again, { target: { value: "Reel" } });
    fireEvent.keyDown(again, { key: "Enter" });
    await act(async () => {});
    expect(transport.requests.map((r) => [r.method, r.params])).toEqual([
      ["sequence.rename", { sequence: B, name: "Reel" }],
    ]);
  });

  it("the x closes a tab without a document command; the last tab has none", () => {
    mount();
    const close = () =>
      [...document.querySelectorAll<HTMLElement>(".cw-seqtabs__close")].map(
        (b) => b.closest<HTMLElement>("[data-sequence]")?.dataset.sequence,
      );
    expect(close()).toEqual([A, B, C]);
    const button = document.querySelector<HTMLElement>(`[data-sequence="${B}"] .cw-seqtabs__close`);
    if (button === null) throw new Error("no close button");
    fireEvent.click(button);
    expect(tabNames()).toEqual(["Main", "Square"]);
    fireEvent(
      screen.getByRole("tab", { name: "Square" }),
      new MouseEvent("auxclick", { bubbles: true, button: 1 }),
    );
    expect(tabNames()).toEqual(["Main"]);
    expect(close()).toEqual([]);
    expect(transport.requests).toEqual([]);
    // The sequences are still in the project.
    expect(Object.keys(docStore.getState().project?.sequences ?? {})).toEqual([A, B, C]);
  });

  it("a tab dropped on another moves there", () => {
    mount();
    const data = { setData() {}, effectAllowed: "", dropEffect: "" };
    fireEvent.dragStart(screen.getByRole("tab", { name: "Main" }), { dataTransfer: data });
    fireEvent.drop(screen.getByRole("tab", { name: "Square" }), { dataTransfer: data });
    expect(tabNames()).toEqual(["Vertical", "Square", "Main"]);
    fireEvent.dragStart(screen.getByRole("tab", { name: "Main" }), { dataTransfer: data });
    fireEvent.drop(screen.getByRole("tab", { name: "Vertical" }), { dataTransfer: data });
    expect(tabNames()).toEqual(["Main", "Vertical", "Square"]);
  });

  it("an agent choosing a closed sequence gives it its tab back", () => {
    viewStore.getState().setSequenceTabs({ order: [], closed: [C] });
    mount();
    expect(tabNames()).toEqual(["Main", "Vertical"]);
    act(() => viewStore.getState().applyEngineView({ active_sequence: C }));
    expect(tabNames()).toEqual(["Main", "Vertical", "Square"]);
    expect(selected()).toEqual(["Square"]);
  });
});
