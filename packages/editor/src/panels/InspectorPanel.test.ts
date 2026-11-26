// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-1M.07 component tests: what the inspector shows for a clip, several clips and a media asset,
// and the engine commands its inputs send. The engine is a recording fake here; the same edits run
// against a real engine in `session/session.integration.test.ts`.

import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { i18n } from "../i18n/index.ts";
import { docStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { createFakeEngine, type FakeEngine } from "../testing/fakeEngine.ts";
import { sampleProject, sec } from "../testing/fixtures.ts";
import { InspectorPanel } from "./InspectorPanel.tsx";

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
});

let engine: FakeEngine;

function load(edits: Record<string, object> = {}) {
  docStore.getState().resync({ rev: 1, committed_rev: 1, project: sampleProject(edits) } as never);
}

const selectItems = (...ids: string[]) =>
  viewStore.setState({ selection: ids.map((id) => ({ kind: "item", id }) as never) });

function renderPanel() {
  return render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(InspectorPanel))));
}

beforeEach(() => {
  viewStore.setState({ ...initialViewState, layouts: {} });
  toastStore.getState().clear();
  engine = createFakeEngine();
  engine.install();
  load();
});

afterEach(() => {
  cleanup();
  engine.uninstall();
});

describe("InspectorPanel", () => {
  it("shows the empty state and the project when nothing is selected", () => {
    renderPanel();
    expect(screen.getByText("Nothing selected")).toBeTruthy();
  });

  it("shows the clip's fields as timecodes and values", () => {
    selectItems("itm_v");
    renderPanel();
    expect((screen.getByRole("textbox", { name: "Start" }) as HTMLInputElement).value).toBe(
      "00:00:01:00",
    );
    expect((screen.getByRole("textbox", { name: "Duration" }) as HTMLInputElement).value).toBe(
      "00:00:04:00",
    );
    expect(screen.getByRole("slider", { name: "Opacity" }).getAttribute("aria-valuenow")).toBe(
      "100",
    );
    for (const name of ["Position X", "Position Y", "Scale X", "Rotation", "Gain"]) {
      expect(screen.getByRole("spinbutton", { name }), name).toBeTruthy();
    }
    expect(screen.getByRole("spinbutton", { name: "Scale X" }).getAttribute("aria-valuenow")).toBe(
      "100",
    );
  });

  it("sends item.move for a valid start, in one batch with the undo name", async () => {
    selectItems("itm_v");
    renderPanel();
    const user = userEvent.setup();
    const start = screen.getByRole("textbox", { name: "Start" });
    await user.clear(start);
    await user.type(start, "00:00:02:00{Enter}");
    await waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    expect(engine.names()).toEqual(["edit.begin_tx", "item.move", "edit.commit_tx"]);
    expect(engine.calls[0]?.input).toEqual({ kind: "batch", label: "Move clip" });
    expect(engine.calls[1]?.input).toEqual({ item: "itm_v", start: sec(2) });
  });

  it("sends clip.trim on the end edge for a new duration", async () => {
    selectItems("itm_v");
    renderPanel();
    const user = userEvent.setup();
    const duration = screen.getByRole("textbox", { name: "Duration" });
    await user.clear(duration);
    await user.type(duration, "2s{Enter}");
    await waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    expect(engine.calls[1]).toMatchObject({
      name: "clip.trim",
      input: { item: "itm_v", edge: "end", to: sec(3) },
    });
    expect(engine.calls[0]?.input).toEqual({ kind: "batch", label: "Trim clip" });
  });

  it("refuses an input that is not a time and says how to write one", async () => {
    selectItems("itm_v");
    renderPanel();
    const user = userEvent.setup();
    const start = screen.getByRole("textbox", { name: "Start" });
    await user.clear(start);
    await user.type(start, "soon{Enter}");
    expect((await screen.findByRole("alert")).textContent).toMatch(/Not a time/);
    expect(start.getAttribute("aria-invalid")).toBe("true");
    expect(engine.calls).toEqual([]);
    // Escape puts the old value back.
    await user.keyboard("{Escape}");
    expect((start as HTMLInputElement).value).toBe("00:00:01:00");
  });

  it("does not send anything when the typed time is the current one", async () => {
    selectItems("itm_v");
    renderPanel();
    const user = userEvent.setup();
    const start = screen.getByRole("textbox", { name: "Start" });
    await user.clear(start);
    await user.type(start, "1s{Enter}");
    expect(engine.calls).toEqual([]);
  });

  it("after an opacity key press the slider follows the document again (an undo)", async () => {
    selectItems("itm_v");
    renderPanel();
    const thumb = screen.getByRole("slider", { name: "Opacity" });
    act(() => thumb.focus());
    fireEvent.keyDown(thumb, { key: "ArrowLeft" });
    await waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    const opacity = (value: number) => ({
      visual: {
        transform: {
          position: { x: 0, y: 0 },
          scale: { x: 1, y: 1 },
          rotation: 0,
          anchor: { x: 0, y: 0 },
        },
        opacity: value,
      },
    });
    act(() => load({ itm_v: opacity(0.99) }));
    await waitFor(() =>
      expect(screen.getByRole("slider", { name: "Opacity" }).getAttribute("aria-valuenow")).toBe(
        "99",
      ),
    );
    // The edit is undone: the document says 100 again, and the slider must show it.
    act(() => load({ itm_v: opacity(1) }));
    await waitFor(() =>
      expect(screen.getByRole("slider", { name: "Opacity" }).getAttribute("aria-valuenow")).toBe(
        "100",
      ),
    );
  });

  it("a second key press starts from the first one's result, before the document catches up", async () => {
    selectItems("itm_v");
    renderPanel();
    const rotation = screen.getByRole("spinbutton", { name: "Rotation" });
    act(() => rotation.focus());
    fireEvent.keyDown(rotation, { key: "ArrowUp" });
    fireEvent.keyDown(rotation, { key: "ArrowUp", shiftKey: true });
    await waitFor(() =>
      expect(engine.names().filter((n) => n === "edit.commit_tx")).toHaveLength(2),
    );
    const sent = engine.calls
      .filter((c) => c.name === "item.set_props")
      .map(
        (c) => (c.input as { props: { transform: { rotation: number } } }).props.transform.rotation,
      );
    expect(sent).toEqual([1, 11]);
  });

  it("renames a clip with item.set_props", async () => {
    selectItems("itm_v");
    renderPanel();
    const user = userEvent.setup();
    await user.type(screen.getByRole("textbox", { name: "Name" }), "Interview{Enter}");
    await waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    expect(engine.calls[1]).toMatchObject({
      name: "item.set_props",
      input: { item: "itm_v", props: { name: "Interview", opacity: null } },
    });
  });

  it("an opacity key press is one undo entry and does not start a second edit", async () => {
    selectItems("itm_v");
    renderPanel();
    const thumb = screen.getByRole("slider", { name: "Opacity" });
    act(() => thumb.focus());
    fireEvent.keyDown(thumb, { key: "ArrowLeft" });
    await waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    // The slider reports the commit before the change and echoes the value later: neither may
    // open another transaction.
    await new Promise((r) => setTimeout(r, 50));
    expect(engine.names()).toEqual(["edit.begin_tx", "item.set_props", "edit.commit_tx"]);
    expect(engine.calls[0]?.input).toEqual({ kind: "batch", label: "Change opacity" });
    expect(engine.calls[1]).toMatchObject({
      name: "item.set_props",
      input: { items: ["itm_v"], props: { opacity: 0.99 }, at: 0 },
      meta: { tx: expect.stringMatching(/^tx_/) },
    });
  });

  it("a scrubber drag is one gesture: begin, live set_props with the tx, one commit", async () => {
    selectItems("itm_v");
    renderPanel();
    const rotation = screen.getByRole("spinbutton", { name: "Rotation" });
    fireEvent.pointerDown(rotation, { clientX: 100, button: 0, pointerId: 1 });
    for (const x of [110, 120, 130, 140, 150]) {
      fireEvent.pointerMove(rotation, { clientX: x, pointerId: 1 });
      await new Promise((r) => setTimeout(r, 25));
    }
    fireEvent.pointerUp(rotation, { clientX: 150, pointerId: 1 });
    await waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    const names = engine.names();
    expect(names[0]).toBe("edit.begin_tx");
    expect(names.filter((n) => n === "edit.begin_tx")).toHaveLength(1);
    expect(names.filter((n) => n === "edit.commit_tx")).toHaveLength(1);
    expect(names.at(-1)).toBe("edit.commit_tx");
    expect(engine.calls[0]?.input).toEqual({ kind: "gesture", label: "Change rotation" });
    const sets = engine.calls.filter((c) => c.name === "item.set_props");
    expect(sets.length).toBeGreaterThan(0);
    for (const call of sets) expect(call.meta).toEqual({ tx: expect.stringMatching(/^tx_/) });
    const last = sets.at(-1)?.input as { props: { transform: { rotation: number } } };
    expect(last.props.transform.rotation).toBeGreaterThan(0);
  });

  it("scale scrubs both axes together, as a percentage", async () => {
    selectItems("itm_v");
    renderPanel();
    const scale = screen.getByRole("spinbutton", { name: "Scale X" });
    act(() => scale.focus());
    fireEvent.keyDown(scale, { key: "ArrowUp" });
    await waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    expect(engine.calls[1]).toMatchObject({
      name: "item.set_props",
      input: { items: ["itm_v"], props: { transform: { scale: { x: 1.01, y: 1.01 } } } },
    });
  });

  it("shows mixed values for several clips and edits them as one step", async () => {
    load({
      itm_a: {
        visual: {
          transform: {
            position: { x: 0, y: 0 },
            scale: { x: 1, y: 1 },
            rotation: 0,
            anchor: { x: 0, y: 0 },
          },
          opacity: 0.5,
        },
      },
    });
    selectItems("itm_v", "itm_a");
    renderPanel();
    expect(screen.getByText(/2 clips selected/)).toBeTruthy();
    expect(screen.getByRole("slider", { name: "Opacity" }).getAttribute("aria-valuetext")).toBe(
      "—",
    );
    const start = screen.getByRole("textbox", { name: "Start" }) as HTMLInputElement;
    expect(start.disabled).toBe(true);
    expect(start.placeholder).toBe("Mixed");
    // The gain is the same on both clips: no placeholder.
    expect(
      screen.getByRole("spinbutton", { name: "Gain" }).getAttribute("aria-valuetext"),
    ).not.toBe("mixed");

    const rotation = screen.getByRole("spinbutton", { name: "Rotation" });
    act(() => rotation.focus());
    fireEvent.keyDown(rotation, { key: "ArrowUp" });
    await waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    const sets = engine.calls.filter((c) => c.name === "item.set_props");
    // One command for both clips.
    expect(sets).toHaveLength(1);
    expect((sets[0]?.input as { items: string[] } | undefined)?.items).toEqual(["itm_v", "itm_a"]);
    expect(engine.names().filter((n) => n === "edit.begin_tx")).toHaveLength(1);
  });

  it("disables the fields of a clip on a locked track", () => {
    const project = sampleProject();
    const sequences = project.sequences as unknown as Record<
      string,
      { tracks: Record<string, { locked: boolean }> }
    >;
    const lane = sequences.seq_1?.tracks.trk_v;
    if (lane === undefined) throw new Error("fixture has no trk_v");
    lane.locked = true;
    docStore.getState().resync({ rev: 1, committed_rev: 1, project } as never);
    selectItems("itm_v");
    renderPanel();
    expect((screen.getByRole("textbox", { name: "Name" }) as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByText(/track is locked/i)).toBeTruthy();
  });

  it("shows the probe info of a media asset selected in the bin", () => {
    viewStore.setState({ selection: [{ kind: "media", id: "med_a" } as never] });
    renderPanel();
    const panel = screen.getByTestId("media-inspector");
    expect(within(panel).getByText("beach.mp4")).toBeTruthy();
    expect(within(panel).getByText("1920 × 1080")).toBeTruthy();
    expect(within(panel).getByText("30 fps")).toBeTruthy();
    expect(within(panel).getByText("48000 Hz, 2 ch")).toBeTruthy();
    expect(within(panel).getByText("00:00:10:00")).toBeTruthy();
    expect(within(panel).getByText(/Placeholder data/)).toBeTruthy();
  });

  it("toasts the engine's refusal and leaves the field as it was", async () => {
    engine.reject(
      "item.move",
      Object.assign(new Error("clip overlaps"), { code: "invalid", path: "/start" }),
    );
    selectItems("itm_v");
    renderPanel();
    const user = userEvent.setup();
    const start = screen.getByRole("textbox", { name: "Start" });
    await user.clear(start);
    await user.type(start, "00:00:02:00{Enter}");
    await waitFor(() => expect(toastStore.getState().notices).toHaveLength(1));
    expect(toastStore.getState().notices[0]).toMatchObject({
      tone: "danger",
      title: "Change not applied",
    });
    expect(engine.names()).toContain("edit.cancel_tx");
  });

  it("scrubbing one axis of the position is one gesture that sends both axes", async () => {
    selectItems("itm_v");
    renderPanel();
    const y = screen.getByRole("spinbutton", { name: "Position Y" });
    fireEvent.pointerDown(y, { clientX: 100, button: 0, pointerId: 1 });
    for (const x of [110, 120, 130, 140, 150]) {
      fireEvent.pointerMove(y, { clientX: x, pointerId: 1 });
      await new Promise((r) => setTimeout(r, 25));
    }
    fireEvent.pointerUp(y, { clientX: 150, pointerId: 1 });
    await waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    expect(engine.calls[0]?.input).toEqual({ kind: "gesture", label: "Change position" });
    const sets = engine.calls.filter((c) => c.name === "item.set_props");
    const last = sets.at(-1)?.input as {
      props: { transform: { position: { x: number; y: number } } };
    };
    expect(last.props.transform.position.x).toBe(0);
    expect(last.props.transform.position.y).toBeGreaterThan(0);
  });
});
