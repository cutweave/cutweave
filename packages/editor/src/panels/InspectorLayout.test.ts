// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The Inspector's Layout section (WP-2C.08): with two to four video clips selected it shows the
// layouts for that many clips as small diagram buttons; a choice is one `item.apply_layout`, the
// fill buttons re-apply the last layout, and Swap reverses the order of two clips.
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { forgetAppliedLayouts } from "../layout/actions.ts";
import { docStore } from "../state/docStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { sampleProject, sec } from "../testing/fixtures.ts";
import { InspectorPanel } from "./InspectorPanel.tsx";

let call: ReturnType<typeof vi.fn>;

function connect(): void {
  call = vi.fn(async (name: string) => {
    if (name === "item.apply_layout") return { cells: [{ item: "itm_v" }, { item: "itm_w" }] };
    if (name === "media.proxy_status") return { proxies: [] };
    if (name === "keyframe.eval") return { samples: [] };
    return {};
  });
  engineStore.setState({
    rpc: { call, on: () => () => undefined } as never,
    connection: "ready",
    blocked: null,
  });
}

const calls = (command: string) =>
  call.mock.calls.filter(([name]) => name === command).map(([, input]) => input);

/** The sample project plus a second video clip `itm_w` on its own track. */
function twoVideos(): void {
  const project = sampleProject() as unknown as {
    sequences: Record<
      string,
      {
        track_order: string[];
        tracks: Record<string, { items: Record<string, unknown> } & Record<string, unknown>>;
      }
    >;
  };
  const seq = Object.values(project.sequences)[0];
  if (seq === undefined) throw new Error("no sequence");
  const base = seq.tracks.trk_v;
  const clip = base?.items.itm_v as Record<string, unknown>;
  seq.tracks.trk_w = { ...base, id: "trk_w", items: { itm_w: { ...clip, id: "itm_w" } } };
  seq.track_order.unshift("trk_w");
  docStore.setState({ project: project as never, status: "ready" } as never);
}

function select(...ids: string[]): void {
  viewStore.setState({
    selection: ids.map((id) => ({ kind: "item", id })),
    playhead: sec(2),
  } as never);
}

const view = () => render(h(I18nextProvider, { i18n }, h(InspectorPanel)));

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  connect();
  forgetAppliedLayouts();
});

afterEach(() => {
  cleanup();
  engineStore.setState({ rpc: null, connection: "none" });
});

describe("the Layout section", () => {
  it("is not there for one clip, or for a clip and its sound", () => {
    docStore.setState({ project: sampleProject(), status: "ready" } as never);
    select("itm_v");
    view();
    expect(screen.queryByTestId("layout-section")).toBeNull();
    cleanup();
    select("itm_v", "itm_a");
    view();
    expect(screen.queryByTestId("layout-section")).toBeNull();
  });

  it("shows the layouts for two clips as named diagram buttons", () => {
    twoVideos();
    select("itm_v", "itm_w");
    view();
    const group = screen.getByRole("group", { name: "Choose a layout" });
    for (const name of [
      "Side by side",
      "Top and bottom",
      "Picture in picture, top left",
      "Picture in picture, bottom right",
    ]) {
      expect(within(group).getByRole("button", { name })).toBeTruthy();
    }
    expect(within(group).queryByRole("button", { name: "2 × 2 grid" })).toBeNull();
  });

  it("applies a layout as one item.apply_layout, filling the cells by default", async () => {
    twoVideos();
    select("itm_v", "itm_w");
    view();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Side by side" }));
    await waitFor(() => expect(calls("item.apply_layout")).toHaveLength(1));
    expect(calls("item.apply_layout")[0]).toEqual({
      items: ["itm_v", "itm_w"],
      layout: "side_by_side",
      fill: "crop",
      at: sec(2),
    });
  });

  it("Swap waits for a layout, then reverses the cell order; Fit re-applies it fitted", async () => {
    twoVideos();
    select("itm_v", "itm_w");
    view();
    const user = userEvent.setup();
    const swap = screen.getByRole("button", { name: "Swap" });
    expect((swap as HTMLButtonElement).disabled).toBe(true);
    await user.click(screen.getByRole("button", { name: "Side by side" }));
    await waitFor(() => expect((swap as HTMLButtonElement).disabled).toBe(false));
    await user.click(swap);
    await waitFor(() => expect(calls("item.apply_layout")).toHaveLength(2));
    expect(calls("item.apply_layout")[1]).toMatchObject({
      layout: "side_by_side",
      order: ["itm_w", "itm_v"],
    });
    await user.click(screen.getByRole("radio", { name: "Fit inside" }));
    await waitFor(() => expect(calls("item.apply_layout")).toHaveLength(3));
    expect(calls("item.apply_layout")[2]).toMatchObject({ layout: "side_by_side", fill: "fit" });
  });
});
