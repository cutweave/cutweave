// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Issue #244: while a clip is dragged on the timeline the document changes on every update (the
// clip's `start`), and the panels beside the timeline re-rendered whole each time. These tests
// count renders per drag update: the transport bar and the bin must not render at all, the
// inspector only what shows the moving clip's timing.
import type { DocPatched, PatchOp, Project } from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, render, screen } from "@testing-library/react";
import { createElement as h, Profiler, type ReactElement } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../i18n/index.ts";
import { docStore } from "../state/docStore.ts";
import { initialPlaybackState, playbackStore } from "../state/playbackStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { createFakeEngine, type FakeEngine } from "../testing/fakeEngine.ts";
import { sampleProject, sec } from "../testing/fixtures.ts";
import { InspectorPanel } from "./InspectorPanel.tsx";
import { MediaPanel } from "./MediaPanel.tsx";
import { TransportBar } from "./TransportBar.tsx";

const renders = vi.hoisted(() => ({ section: 0, row: 0, form: 0 }));

// Counts what the inspector draws: sections, their rows and the schema forms.
vi.mock("@cutweave/ui", async (importOriginal) => {
  const ui = await importOriginal<typeof import("@cutweave/ui")>();
  const { createElement } = await import("react");
  const counted = (key: keyof typeof renders, Original: unknown) => (props: object) => {
    renders[key] += 1;
    return createElement(Original as never, props);
  };
  return {
    ...ui,
    InspectorSection: counted("section", ui.InspectorSection),
    InspectorRow: counted("row", ui.InspectorRow),
    SchemaForm: counted("form", ui.SchemaForm),
  };
});

const UPDATES = 5;

/** A drag update: the engine moved the clip (`item.move` inside the gesture's transaction). */
const movePatch = (rev: number, id: string, old: number, start: number): DocPatched =>
  ({
    rev,
    prev_rev: rev - 1,
    committed_rev: 1,
    actor: "user",
    tx_id: "tx_drag",
    cause: "command",
    request_id: null,
    ops: [
      { op: "replace", path: `/sequences/seq_1/tracks/trk_v/items/${id}/start`, old, value: start },
    ] as unknown as PatchOp[],
  }) as unknown as DocPatched;

/** Runs `UPDATES` drag updates of the clip. */
function drag(id = "itm_v"): void {
  let at = sec(1);
  for (let i = 0; i < UPDATES; i++) {
    const next = at + sec(0.1);
    const rev = 2 + i;
    act(() => {
      expect(docStore.getState().applyEvent(movePatch(rev, id, at, next))).toBe("applied");
    });
    at = next;
  }
}

const load = (project: Project): void =>
  docStore.getState().resync({ rev: 1, committed_rev: 1, project });

let engine: FakeEngine;

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
  viewStore.setState({ ...initialViewState, layouts: {} });
  playbackStore.setState({ ...initialPlaybackState });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  engine = createFakeEngine();
  engine.install();
  load(sampleProject());
});

afterEach(() => {
  cleanup();
  engine.uninstall();
});

/** Mounts `panel` inside a Profiler and returns how many commits it has had. */
function mount(panel: ReactElement) {
  const commits = { n: 0 };
  const onRender = (): void => {
    commits.n += 1;
  };
  render(
    h(
      I18nextProvider,
      { i18n },
      h(TooltipProvider, null, h(Profiler, { id: "panel", onRender }, panel)),
    ),
  );
  return commits;
}

describe("a clip drag does not redraw the other panels (#244)", () => {
  it("leaves the transport bar alone", () => {
    const commits = mount(h(TransportBar));
    const before = commits.n;
    drag();
    expect(commits.n - before).toBe(0);
  });

  it("leaves the media bin alone", () => {
    const measured = vi.spyOn(Element.prototype, "getBoundingClientRect");
    const commits = mount(h(MediaPanel));
    const before = commits.n;
    measured.mockClear();
    drag();
    expect(commits.n - before).toBe(0);
    expect(measured).not.toHaveBeenCalled();
    measured.mockRestore();
  });

  it("redraws only the moving clip's timing in the inspector", () => {
    viewStore.setState({ selection: [{ kind: "item", id: "itm_v" }] } as never);
    mount(h(InspectorPanel));
    renders.section = 0;
    renders.row = 0;
    renders.form = 0;
    drag();
    // The Timing section (Start and Duration) follows the clip; nothing else is rebuilt.
    expect(renders.form).toBe(0);
    expect(renders.section / UPDATES).toBeLessThanOrEqual(1);
    expect(renders.row / UPDATES).toBeLessThanOrEqual(2);
    // ...and it does follow the clip: 1 s plus five steps of 0.1 s.
    expect((screen.getByRole("textbox", { name: "Start" }) as HTMLInputElement).value).toBe(
      "00:00:01:15",
    );
  });
});
