// SPDX-License-Identifier: GPL-3.0-or-later
// The sequence tab's menu (`sequence.tab`, WP-2B.03): its rows, what each sends, when a row is
// off and why, and the commands the palette lists for the sequence that shows.

import { MockTransport, type Project, RpcClient, type SequenceId } from "@cutweave/protocol";
import type { MenuEntry, MenuItem } from "@cutweave/ui";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { engineStore, initialEngineState } from "../../engine/engineStore.ts";
import { i18n } from "../../i18n/index.ts";
import { sequenceUiStore } from "../../sequences/uiStore.ts";
import { docStore } from "../../state/docStore.ts";
import { selectActiveSequenceId } from "../../state/selectors.ts";
import { bindShownSequence } from "../../state/shownSequence.ts";
import { initialViewState, NO_SEQUENCE_TABS, viewStore } from "../../state/viewStore.ts";
import { sampleProject } from "../../testing/fixtures.ts";
import { buildMenu } from "../build.ts";
import { registerBuiltinMenus } from "../builtins.ts";
import { createMenuServices, type MenuServices } from "../services.ts";
import { registerSurfaceMenus } from "./index.ts";
import { SEQUENCE_PALETTE_COMMANDS } from "./sequence.ts";

const A = "seq_1" as SequenceId;
const B = "seq_2" as SequenceId;

function projectWith(...ids: SequenceId[]): Project {
  const project = sampleProject();
  const first = project.sequences[A];
  if (first === undefined) throw new Error("the fixture has no seq_1");
  const more = Object.fromEntries(
    ids.map((id) => [id, { ...first, id, name: "Vertical", track_order: [], tracks: {} }]),
  );
  return { ...project, sequences: { ...project.sequences, ...more } } as Project;
}

let services: MenuServices;
let transport: MockTransport;
let off: () => void;
let pending: Promise<void>[] = [];

beforeEach(() => {
  viewStore.setState({ ...initialViewState, layouts: {}, sequenceTabs: NO_SEQUENCE_TABS });
  sequenceUiStore.setState({ renaming: null, settings: null });
  transport = new MockTransport();
  engineStore.setState({
    ...initialEngineState,
    rpc: new RpcClient(transport),
    connection: "ready",
  });
  docStore.getState().resync({ rev: 1, committed_rev: 1, project: projectWith(B) });
  off = bindShownSequence(viewStore, docStore);
  services = createMenuServices({
    t: (label) => i18n.t(label.key, label.vars ?? {}) as string,
  });
  registerBuiltinMenus(services);
  registerSurfaceMenus(services);
  pending = [];
});

afterEach(() => {
  off();
  engineStore.setState({ ...initialEngineState });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
});

const isItem = (entry: MenuEntry): entry is MenuItem =>
  entry.kind === undefined || entry.kind === "item";

function menuFor(sequence: SequenceId) {
  const menu = buildMenu(
    services,
    { surface: "sequence.tab", ids: [sequence], name: "Main" },
    (run) => void pending.push(Promise.resolve(run())),
  );
  if (menu === null) throw new Error("no sequence.tab menu");
  const rows = menu.entries.flatMap((e) => (isItem(e) ? [e] : []));
  const row = (label: string): MenuItem => {
    const found = rows.find((r) => r.label === label);
    if (found === undefined) throw new Error(`no row ${label}`);
    return found;
  };
  const aspect = menu.entries.find((e) => e.kind === "submenu");
  const sizes = aspect?.kind === "submenu" ? aspect.items.filter(isItem) : [];
  return { menu, rows, row, sizes };
}

const choose = async (item: MenuItem): Promise<void> => {
  item.onSelect?.();
  await Promise.all(pending);
};
const sent = () => transport.requests.map((r) => [r.method, r.params]);

describe("the sequence tab's menu", () => {
  it("lists Rename, Duplicate, Settings, Aspect, the tab moves, Close and Delete, named for the tab", () => {
    const { menu, rows, sizes } = menuFor(A);
    expect(menu.name).toBe("Sequence actions for Main");
    expect(rows.map((r) => r.label)).toEqual([
      "Rename",
      "Duplicate",
      "Sequence settings…",
      "Move left",
      "Move right",
      "Close tab",
      "Close other tabs",
      "Delete sequence",
    ]);
    expect(sizes.map((s) => s.label)).toEqual([
      "16:9 (1920 × 1080)",
      "16:9 (3840 × 2160)",
      "9:16 (1080 × 1920)",
      "1:1 (1080 × 1080)",
      "4:5 (1080 × 1350)",
      "21:9 (2560 × 1080)",
    ]);
    // The size the sequence has is the checked one.
    expect(sizes.filter((s) => s.checked === true).map((s) => s.label)).toEqual([
      "16:9 (1920 × 1080)",
    ]);
    expect(rows.find((r) => r.label === "Delete sequence")?.variant).toBe("danger");
  });

  it("an Aspect choice is one sequence.set_format on the tab's sequence, shown or not", async () => {
    transport.handle("sequence.set_format", () => ({}));
    const vertical = menuFor(B).sizes.find((s) => s.label === "9:16 (1080 × 1920)");
    if (vertical === undefined) throw new Error("no 9:16");
    await choose(vertical);
    expect(sent()).toEqual([
      ["sequence.set_format", { sequence: B, width: 1080, height: 1920, reframe: "fit" }],
    ]);
    // The menu was on the other tab: the sequence that shows did not change.
    expect(selectActiveSequenceId(docStore.getState().project)).toBe(A);
  });

  it("Rename starts the edit in the tab, Settings opens the dialog, Duplicate and Delete send their command", async () => {
    transport
      .handle("sequence.duplicate", () => ({ sequence: "seq_3", name: "Vertical copy" }))
      .handle("sequence.delete", () => ({}));
    await choose(menuFor(B).row("Rename"));
    expect(sequenceUiStore.getState().renaming).toBe(B);
    await choose(menuFor(B).row("Sequence settings…"));
    expect(sequenceUiStore.getState().settings).toBe(B);
    await choose(menuFor(B).row("Duplicate"));
    await choose(menuFor(B).row("Delete sequence"));
    expect(sent()).toEqual([
      ["sequence.duplicate", { sequence: B }],
      ["sequence.delete", { sequence: B }],
    ]);
  });

  it("says why a row is off: the first and last tab, the last open tab, the last sequence", () => {
    const first = menuFor(A);
    expect(first.row("Move left")).toMatchObject({
      disabled: true,
      disabledReason: "This is the first tab",
    });
    expect(first.row("Move right").disabled).not.toBe(true);
    expect(menuFor(B).row("Move right")).toMatchObject({
      disabled: true,
      disabledReason: "This is the last tab",
    });
    expect(first.row("Close tab").disabled).not.toBe(true);
    // One tab left open: it stays, and the sequence behind the closed tab can still be deleted.
    viewStore.getState().setSequenceTabs({ order: [], closed: [B] });
    const last = menuFor(A);
    expect(last.row("Close tab")).toMatchObject({
      disabled: true,
      disabledReason: "The last tab stays open",
    });
    expect(last.row("Close other tabs").disabled).toBe(true);
    expect(last.row("Delete sequence").disabled).not.toBe(true);
    // One sequence in the project: it cannot be deleted.
    docStore.getState().resync({ rev: 2, committed_rev: 2, project: projectWith() });
    expect(menuFor(A).row("Delete sequence")).toMatchObject({
      disabled: true,
      disabledReason: "A project keeps at least one sequence",
    });
  });

  it("Close and Move change the tabs only", async () => {
    await choose(menuFor(B).row("Move left"));
    expect(viewStore.getState().sequenceTabs.order).toEqual([B, A]);
    await choose(menuFor(B).row("Close tab"));
    expect(viewStore.getState().sequenceTabs.closed).toEqual([B]);
    expect(sent()).toEqual([]);
  });
});

describe("the palette", () => {
  it("lists the sequence commands that need no tab: they act on the sequence that shows", () => {
    for (const id of SEQUENCE_PALETTE_COMMANDS) {
      expect(services.ui.get(id)?.hidden, id).toBe(false);
    }
    // The ones that only make sense on a tab's menu stay out of it.
    for (const id of ["sequences.set_aspect", "sequences.move_left", "sequences.move_right"]) {
      expect(services.ui.get(id)?.hidden, id).toBe(true);
    }
  });

  it("run from the palette (no target), Sequence settings… and Rename take the sequence that shows", async () => {
    viewStore.getState().setActiveSequence(B);
    const ctx = {
      target: { surface: "palette", ids: [] },
      ids: [],
      call: () => Promise.resolve({}),
    };
    await services.ui.get("sequences.settings")?.run({}, ctx);
    expect(sequenceUiStore.getState().settings).toBe(B);
    await services.ui.get("sequences.next")?.run({}, ctx);
    expect(selectActiveSequenceId(docStore.getState().project)).toBe(A);
  });
});
