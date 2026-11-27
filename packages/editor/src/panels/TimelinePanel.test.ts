// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The timeline panel's data path: the engine's sequence reaches the `packages/timeline` renderer
// through `docStore` selectors, and follows `doc.patched` events. The canvas itself is stubbed
// (jsdom has no Canvas2D); what matters here is the model the renderer receives.
import type { DocPatched, PatchOp, Project } from "@cutweave/protocol";
import type { TimelineCanvasProps, TimelinePointerEvent } from "@cutweave/timeline";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, render, screen } from "@testing-library/react";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { docStore } from "../state/docStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { TimelinePanel } from "./TimelinePanel.tsx";

const seen: TimelineCanvasProps[] = [];

vi.mock("@cutweave/timeline", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@cutweave/timeline")>()),
  TimelineCanvas: (props: TimelineCanvasProps) => {
    seen.push(props);
    return h("div", {
      role: "application",
      "aria-label": props["aria-label"],
      "data-items": props.model.items.map((i) => i.id).join(","),
    });
  },
}));

/** The design system's icon buttons drawn, by label (what a drag must not draw again, #241). */
const buttonDraws = new Map<string, number>();

vi.mock("@cutweave/ui", async (importOriginal) => {
  const ui = await importOriginal<typeof import("@cutweave/ui")>();
  // React 19 passes `ref` as a prop: the counted button forwards it with the rest.
  const IconButton = (props: Parameters<typeof ui.IconButton>[0]) => {
    buttonDraws.set(props.label, (buttonDraws.get(props.label) ?? 0) + 1);
    return h(ui.IconButton, props);
  };
  return { ...ui, IconButton };
});

const clip = (id: string, start: number) => ({
  kind: "clip",
  id,
  name: null,
  start,
  duration: 705_600_000,
  enabled: true,
  linked_group: null,
  attached_to: null,
  markers: {},
  media: "med_a",
  source_in: 0,
  audio: { gain: 0 },
  visual: { transform: {}, opacity: 1 },
  custom: {},
});

const project = (items: object[]): Project =>
  ({
    meta: { name: "P" },
    settings: {},
    media: { med_a: { id: "med_a", kind: "video", path_abs: "/m/a.mp4", probe: {} } },
    sequences: {
      seq_1: {
        id: "seq_1",
        name: "Main",
        format: { width: 1920, height: 1080, frame_rate: { num: 30, den: 1 } },
        track_order: ["trk_v"],
        markers: {},
        in_out: null,
        tracks: {
          trk_v: {
            id: "trk_v",
            kind: "video",
            name: "V1",
            items: Object.fromEntries(items.map((i) => [(i as { id: string }).id, i])),
            muted: false,
            hidden: false,
            locked: false,
            solo: false,
            magnetic: false,
            height: null,
            gain: 0,
            custom: {},
          },
        },
        custom: {},
      },
    },
    custom: {},
  }) as unknown as Project;

const insertPatch = (rev: number, id: string, start: number): DocPatched =>
  ({
    rev,
    prev_rev: rev - 1,
    committed_rev: rev,
    actor: "user",
    tx_id: `tx_${rev}`,
    cause: "command",
    request_id: null,
    ops: [
      { op: "add", path: `/sequences/seq_1/tracks/trk_v/items/${id}`, value: clip(id, start) },
    ] as unknown as PatchOp[],
  }) as unknown as DocPatched;

/** A drag update: the engine moved the clip from `old` (`item.move` with `_meta.tx`). */
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
      {
        op: "replace",
        path: `/sequences/seq_1/tracks/trk_v/items/${id}/start`,
        old,
        value: start,
      },
    ] as unknown as PatchOp[],
  }) as unknown as DocPatched;

/** The pointer moving over the canvas where nothing is. */
const hover = (x: number): TimelinePointerEvent => ({
  type: "move",
  x,
  y: 30,
  hit: {
    zone: "none",
    time: 0,
    trackId: null,
    trackIndex: -1,
    itemId: null,
    itemIndex: -1,
    keyframeIndex: -1,
    keyframeTime: null,
    toggle: null,
  },
  button: 0,
  buttons: 0,
  shiftKey: false,
  ctrlKey: false,
  altKey: false,
  metaKey: false,
  original: new MouseEvent("pointermove"),
});

const renderPanel = () =>
  render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(TimelinePanel))));

beforeEach(() => {
  seen.length = 0;
  viewStore.setState({ ...initialViewState, layouts: {} });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  engineStore.setState({ rpc: null, connection: "none" });
});
afterEach(cleanup);

describe("TimelinePanel", () => {
  it("keeps the sample lanes when no engine is attached", () => {
    docStore.getState().resync({ rev: 1, committed_rev: 1, project: project([]) });
    renderPanel();
    expect(screen.getByRole("list", { name: "Tracks" })).toBeTruthy();
    expect(screen.queryByRole("application", { name: "Timeline" })).toBeNull();
  });

  it("keeps the sample lanes while the engine has not delivered a sequence yet", () => {
    engineStore.setState({ connection: "connecting" });
    renderPanel();
    expect(screen.getByRole("list", { name: "Tracks" })).toBeTruthy();
  });

  it("renders the engine's sequence and updates live as doc.patched events arrive", () => {
    engineStore.setState({ connection: "ready" });
    docStore.getState().resync({ rev: 1, committed_rev: 1, project: project([]) });
    renderPanel();
    const canvas = screen.getByRole("application", { name: "Timeline" });
    expect(canvas.getAttribute("data-items")).toBe("");
    const first = seen.at(-1);
    expect(first?.model.tracks.map((t) => t.id)).toEqual(["trk_v"]);

    act(() => {
      expect(docStore.getState().applyEvent(insertPatch(2, "itm_1", 0))).toBe("applied");
    });
    expect(screen.getByRole("application", { name: "Timeline" }).getAttribute("data-items")).toBe(
      "itm_1",
    );
    act(() => {
      docStore.getState().applyEvent(insertPatch(3, "itm_2", 705_600_000));
    });
    const last = seen.at(-1);
    expect(last?.model.items.map((i) => [i.id, i.start])).toEqual([
      ["itm_1", 0],
      ["itm_2", 705_600_000],
    ]);
  });

  it("draws only the canvas again while the pointer moves and a drag moves a clip (#241)", () => {
    engineStore.setState({ connection: "ready" });
    docStore.getState().resync({ rev: 1, committed_rev: 1, project: project([clip("itm_1", 0)]) });
    renderPanel();
    expect(buttonDraws.get("Snapping")).toBeGreaterThan(0);
    expect(buttonDraws.get("Add track")).toBeGreaterThan(0);
    const drawn = seen.length;
    buttonDraws.clear();
    const pointer = seen.at(-1)?.onPointer;
    act(() => {
      for (let x = 200; x < 220; x += 1) pointer?.(hover(x));
    });
    act(() => {
      let at = 0;
      for (let rev = 2; rev <= 6; rev += 1) {
        const patch = movePatch(rev, "itm_1", at, rev * 1_000);
        expect(docStore.getState().applyEvent(patch)).toBe("applied");
        at = rev * 1_000;
      }
    });
    // The canvas has each position...
    expect(seen.length).toBeGreaterThan(drawn);
    expect(seen.at(-1)?.model.items.map((i) => i.start)).toEqual([6_000]);
    // ...and nothing around it was drawn again: not the toolbar, not the track headers' corner.
    expect([...buttonDraws.keys()]).toEqual([]);
  });

  it("feeds zoom, scroll, playhead and item selection from the view store", () => {
    engineStore.setState({ connection: "ready" });
    docStore.getState().resync({ rev: 1, committed_rev: 1, project: project([clip("itm_1", 0)]) });
    viewStore.setState({
      zoom: 250,
      scroll: { x: 10, y: 20 },
      playhead: 123,
      selection: [
        { kind: "item", id: "itm_1" as never },
        { kind: "track", id: "trk_v" as never },
      ],
    });
    renderPanel();
    const state = seen.at(-1)?.viewState;
    expect(state).toMatchObject({ pxPerSec: 250, scrollX: 10, scrollY: 20, playhead: 123 });
    expect([...(state?.selection ?? [])]).toEqual(["itm_1"]);
  });

  it("hands pointer input and middle-button panning to the interaction layer", () => {
    engineStore.setState({ connection: "ready" });
    docStore.getState().resync({ rev: 1, committed_rev: 1, project: project([clip("itm_1", 0)]) });
    renderPanel();
    const props = seen.at(-1);
    expect(props?.pan).toBe("host");
    expect(typeof props?.onPointer).toBe("function");
    expect(typeof props?.onView).toBe("function");
    expect(props?.viewState?.snapGuides).toEqual([]);
    expect(props?.viewState?.marquee).toBeNull();
  });

  it("toggles snapping from the toolbar and with N, and shows its state", () => {
    engineStore.setState({ connection: "ready" });
    docStore.getState().resync({ rev: 1, committed_rev: 1, project: project([clip("itm_1", 0)]) });
    renderPanel();
    const magnet = screen.getByRole("button", { name: "Snapping" });
    expect(magnet.getAttribute("aria-pressed")).toBe("true");
    act(() => magnet.click());
    expect(viewStore.getState().snapping).toBe(false);
    expect(screen.getByRole("button", { name: "Snapping" }).getAttribute("aria-pressed")).toBe(
      "false",
    );
    const canvas = screen.getByRole("application", { name: "Timeline" });
    act(() => {
      canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "n", bubbles: true }));
    });
    expect(viewStore.getState().snapping).toBe(true);
  });

  it("disables split and snapping without a live sequence", () => {
    renderPanel();
    expect(
      (screen.getByRole("button", { name: "Split at playhead" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect((screen.getByRole("button", { name: "Snapping" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it("stores the zoom and scroll the user caused in the view store", () => {
    engineStore.setState({ connection: "ready" });
    docStore.getState().resync({ rev: 1, committed_rev: 1, project: project([]) });
    renderPanel();
    act(() => {
      seen.at(-1)?.onViewChange?.({
        pxPerSec: 400,
        scrollX: 5,
        scrollY: 6,
      } as never);
    });
    expect(viewStore.getState().zoom).toBe(400);
    expect(viewStore.getState().scroll).toEqual({ x: 5, y: 6 });
  });
});
