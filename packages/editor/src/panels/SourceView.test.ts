// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The Preview's Source view (WP-1M.15, ADR-0022): while the engine plays a media item on its own
// the Preview says "Source" with the asset's name, its own timecode and length, the transport bar
// and its keys address the source, Esc and "Back to timeline" return to the sequence, and Enter
// places the asset at the timeline's playhead. jsdom has no canvas: the pictures are covered by
// the user flow (F31).
import { type MediaId, MockTransport, RpcClient } from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { dragStore } from "../media/dragStore.ts";
import { docStore } from "../state/docStore.ts";
import { initialPlaybackState, playbackStore } from "../state/playbackStore.ts";
import { sourceViewStore } from "../state/sourceViewStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { FLICKS, sampleProject } from "../testing/fixtures.ts";
import { useTransportWiring } from "../transport/useTransport.ts";
import { PreviewPanel } from "./PreviewPanel.tsx";

function Wired() {
  useTransportWiring();
  return h(PreviewPanel);
}

const renderPanel = () => render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(Wired))));

const SOURCE = { media: "med_a", duration: 10 * FLICKS, fps: { num: 30, den: 1 } };
const state = (over: object = {}) => ({
  state: "stopped",
  playing: false,
  rate: 0,
  t: 0,
  quality: "full",
  dropped: 0,
  ...over,
});
const sourceState = (over: object = {}) =>
  state({ state: "playing", playing: true, rate: 1, source: SOURCE, ...over });

let mock: MockTransport;
const sent = () => mock.requests.map((r) => [r.method, r.params]);
const header = () => screen.queryByTestId("source-header");

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

beforeEach(() => {
  viewStore.setState({ ...initialViewState, layouts: {} });
  playbackStore.setState({ ...initialPlaybackState });
  sourceViewStore.setState({ host: "preview" });
  mock = new MockTransport();
  for (const method of ["view.play", "view.pause", "view.set_rate", "view.step", "view.seek"]) {
    mock.handle(method, (p) => sourceState({ t: (p as { t?: number }).t ?? 0 }));
  }
  mock.handle("view.set_source", () => state({ t: 0, source: null }));
  mock.handle("timeline.insert", () => ({ clips: ["itm_new"] }));
  mock.handle("timeline.overwrite", () => ({ clips: ["itm_new"] }));
  engineStore.setState({ rpc: new RpcClient(mock), connection: "ready" });
  docStore.getState().resync({ rev: 1, committed_rev: 1, project: sampleProject() });
});
afterEach(cleanup);

const playSource = (over: object = {}) =>
  act(() => playbackStore.getState().applyState(sourceState(over)));

describe("the Preview's Source view", () => {
  it("is the timeline until a source is set, then says Source with name, time and length", () => {
    renderPanel();
    expect(header()).toBeNull();
    expect(screen.getByRole("radiogroup", { name: "Preview aspect ratio" })).toBeTruthy();
    playSource({ t: (2 * FLICKS) / 30 });
    expect(header()).not.toBeNull();
    expect(screen.getByText("Source")).toBeTruthy();
    expect(screen.getByText("beach.mp4")).toBeTruthy();
    // The source's own time at its own rate, with its length (10 s at 30 fps).
    expect(screen.getByTestId("source-timecode").textContent).toBe("00:00:00:02");
    expect(screen.getByTestId("source-duration").textContent).toBe("00:00:10:00");
    // No aspect switch or timeline overlay while it shows, and the transport reads the same time.
    expect(screen.queryByRole("radiogroup", { name: "Preview aspect ratio" })).toBeNull();
    expect(screen.queryByText("Nothing to preview")).toBeNull();
    expect(document.querySelector(".cw-preview__readout")).toBeNull();
    expect(screen.getByLabelText("Current time").textContent).toBe("00:00:00:02");
  });

  it("is off while the Inspector shows the source: the Preview stays on the timeline", () => {
    renderPanel();
    sourceViewStore.getState().setHost("inspector");
    playSource();
    expect(header()).toBeNull();
    expect(screen.getByRole("radiogroup", { name: "Preview aspect ratio" })).toBeTruthy();
  });

  it("the transport's buttons and keys address the source, and the sequence after Back", async () => {
    renderPanel();
    playSource({ state: "stopped", playing: false });
    const sequenceEnd = 5 * FLICKS - FLICKS / 30;
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Go to end" }));
    });
    for (const key of ["ArrowRight", "l", " "]) {
      await act(async () => {
        fireEvent.keyDown(document.body, { key });
      });
    }
    // The end is the source's (10 s), not the sequence's (5 s); the keys send the same commands.
    expect(sent()).toEqual([
      ["view.seek", { t: 10 * FLICKS - FLICKS / 30 }],
      ["view.step", { frames: 1 }],
      ["view.set_rate", { rate: 2 }], // the mock answers "playing at 1x" to the step before
      ["view.pause", {}],
    ]);
    expect(viewStore.getState().playhead).toBe(0);
    mock.requests.length = 0;
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Back to timeline" }));
    });
    expect(sent()).toEqual([["view.set_source", {}]]);
    expect(header()).toBeNull();
    mock.requests.length = 0;
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Go to end" }));
    });
    expect(sent()).toEqual([["view.seek", { t: sequenceEnd }]]);
    expect(viewStore.getState().playhead).toBe(sequenceEnd);
  });

  it("Esc in the Preview goes back to the timeline", async () => {
    renderPanel();
    playSource();
    const stage = screen.getByRole("group", { name: /^Source: beach\.mp4/ });
    await act(async () => {
      fireEvent.keyDown(stage, { key: "Escape" });
    });
    expect(sent()).toEqual([["view.set_source", {}]]);
    expect(header()).toBeNull();
  });

  it("Enter, the comma and the Insert button insert the whole asset at the timeline's playhead", async () => {
    renderPanel();
    viewStore.getState().setPlayhead(2 * FLICKS);
    playSource({ t: 3 * FLICKS });
    const stage = screen.getByRole("group", { name: /^Source: beach\.mp4/ });
    const whole = [
      "timeline.insert",
      {
        sequence: "seq_1",
        track: "trk_v",
        media: "med_a",
        at: 2 * FLICKS,
        source_in: 0,
        duration: 10 * FLICKS,
        mode: "insert",
      },
    ];
    for (const key of ["Enter", ","]) {
      mock.requests.length = 0;
      await act(async () => {
        fireEvent.keyDown(stage, { key });
      });
      expect(sent()).toEqual([whole]);
    }
    // The source keeps playing: the timeline's playhead and the view stay as they were.
    expect(header()).not.toBeNull();
    expect(viewStore.getState().playhead).toBe(2 * FLICKS);
    mock.requests.length = 0;
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Insert at the playhead" }));
    });
    expect(sent()).toEqual([whole]);
  });

  it("I and O mark the source at its playhead; the comma inserts and the period overwrites the range", async () => {
    renderPanel();
    viewStore.getState().setPlayhead(6 * FLICKS);
    playSource({ state: "stopped", playing: false, t: 2 * FLICKS });
    const stage = screen.getByRole("group", { name: /^Source: beach\.mp4/ });
    expect(screen.queryByTestId("source-range")).toBeNull();
    expect(screen.getByRole("button", { name: "Clear in and out" })).toHaveProperty(
      "disabled",
      true,
    );
    await act(async () => {
      fireEvent.keyDown(stage, { key: "i" });
    });
    playSource({ state: "stopped", playing: false, t: 5 * FLICKS });
    await act(async () => {
      fireEvent.keyDown(stage, { key: "o" });
    });
    // View state, not the document: nothing is sent from here (viewSync mirrors it).
    expect(sent()).toEqual([]);
    expect(viewStore.getState().sourceMarks).toEqual({
      med_a: { in: 2 * FLICKS, out: 5 * FLICKS },
    });
    // The range shows on the scrub bar (20% to 50% of 10 s) and in words.
    const range = screen.getByTestId("source-range");
    expect(range.style.insetInlineStart).toBe("20%");
    expect(range.style.inlineSize).toBe("30%");
    expect(screen.getByTestId("source-marks").textContent).toBe(
      "In 00:00:02:00 · Out 00:00:05:00 · 00:00:03:00",
    );
    expect(
      screen.getByRole("slider", { name: "Source position" }).getAttribute("aria-valuetext"),
    ).toBe("00:00:05:00, in 00:00:02:00, out 00:00:05:00");
    const args = {
      sequence: "seq_1",
      track: "trk_v",
      media: "med_a",
      at: 6 * FLICKS,
      source_in: 2 * FLICKS,
      duration: 3 * FLICKS,
    };
    await act(async () => {
      fireEvent.keyDown(stage, { key: "," });
    });
    await act(async () => {
      fireEvent.keyDown(stage, { key: "." });
    });
    expect(sent()).toEqual([
      ["timeline.insert", { ...args, mode: "insert" }],
      ["timeline.overwrite", args],
    ]);
    mock.requests.length = 0;
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Overwrite at the playhead" }));
    });
    expect(sent()).toEqual([["timeline.overwrite", args]]);
    // The buttons mark too, and Clear (Alt+X) removes both.
    playSource({ state: "stopped", playing: false, t: 1 * FLICKS });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Set in point" }));
    });
    expect(viewStore.getState().sourceMarks).toEqual({
      med_a: { in: 1 * FLICKS, out: 5 * FLICKS },
    });
    await act(async () => {
      fireEvent.keyDown(stage, { key: "x", altKey: true });
    });
    expect(viewStore.getState().sourceMarks).toEqual({});
    expect(screen.queryByTestId("source-range")).toBeNull();
  });

  it("the scrub bar seeks the source on its own frames", async () => {
    renderPanel();
    playSource({ state: "stopped", playing: false, t: 2 * FLICKS });
    const bar = screen.getByRole("slider", { name: "Source position" });
    expect(bar.getAttribute("aria-valuenow")).toBe("60");
    expect(bar.getAttribute("aria-valuemax")).toBe("299");
    await act(async () => {
      fireEvent.keyDown(bar, { key: "End" });
    });
    expect(sent()).toEqual([["view.seek", { t: 10 * FLICKS - FLICKS / 30 }]]);
    expect(viewStore.getState().playhead).toBe(0);
  });

  it("dragging the picture carries the marked range; the picture grip leaves the sound behind", () => {
    renderPanel();
    playSource({ state: "stopped", playing: false });
    act(() =>
      viewStore.getState().setSourceMarks("med_a" as MediaId, { in: 2 * FLICKS, out: 5 * FLICKS }),
    );
    const data = { setData: () => undefined, effectAllowed: "" };
    fireEvent.dragStart(screen.getByTestId("source-drag-video"), { dataTransfer: data });
    expect(dragStore.getState().session).toEqual({
      id: "med_a",
      kind: "video",
      duration: 3 * FLICKS,
      range: { sourceIn: 2 * FLICKS, duration: 3 * FLICKS },
      link: false,
    });
    fireEvent.dragStart(screen.getByTestId("source-drag-audio"), { dataTransfer: data });
    expect(dragStore.getState().session).toMatchObject({ kind: "audio", duration: 3 * FLICKS });
    act(() => dragStore.getState().end());
  });

  it("returns to the timeline when the engine ends the source (removed asset, another project)", () => {
    renderPanel();
    playSource();
    expect(header()).not.toBeNull();
    playSource({ source: null, state: "stopped", playing: false });
    expect(header()).toBeNull();
  });

  it("has no axe violations", async () => {
    const { container } = renderPanel();
    playSource();
    expect((await axe.run(container)).violations).toEqual([]);
  });

  it("has no axe violations with marks set", async () => {
    const { container } = renderPanel();
    playSource({ state: "stopped", playing: false, t: 3 * FLICKS });
    act(() =>
      viewStore.getState().setSourceMarks("med_a" as MediaId, { in: 2 * FLICKS, out: 5 * FLICKS }),
    );
    expect(screen.getByTestId("source-range")).toBeTruthy();
    expect((await axe.run(container)).violations).toEqual([]);
  });
});
