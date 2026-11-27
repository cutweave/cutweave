// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The Inspector's Video sections (WP-2H.03a): the controls come from the `item.set_props` schema,
// several clips edit as one command, typed entries take expressions, the diamond keys at the
// playhead, and a property a newer schema adds gets a control with no UI code change.
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { commandCatalog } from "../palette/catalog.ts";
import { docStore } from "../state/docStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { sampleProject, sec } from "../testing/fixtures.ts";
import { InspectorPanel } from "./InspectorPanel.tsx";

let call: ReturnType<typeof vi.fn>;
const original = commandCatalog.getState().commands;

function connect(): void {
  call = vi.fn(async (name: string) => {
    if (name === "edit.begin_tx") return { tx: "tx_1" };
    if (name === "media.proxy_status") return { proxies: [] };
    if (name === "keyframe.eval") return { samples: [] };
    return {};
  });
  const rpc = { call, on: () => () => undefined };
  engineStore.setState({ rpc: rpc as never, connection: "ready", blocked: null });
}

const calls = (command: string) =>
  call.mock.calls.filter(([name]) => name === command).map(([, input]) => input);

const view = () => render(h(I18nextProvider, { i18n }, h(InspectorPanel)));

function select(...ids: string[]): void {
  docStore.setState({ project: sampleProject(), status: "ready" } as never);
  viewStore.setState({
    selection: ids.map((id) => ({ kind: "item", id })),
    playhead: sec(2),
  } as never);
}

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(connect);

afterEach(() => {
  cleanup();
  commandCatalog.setState({ commands: original });
});

describe("the Video sections", () => {
  it("shows transform, crop, flip, opacity and blend from the schema, in sections", () => {
    select("itm_v");
    view();
    for (const name of ["Transform", "Crop", "Flip", "Compositing"]) {
      expect(screen.getByRole("button", { name })).toBeTruthy();
    }
    expect(screen.getByRole("spinbutton", { name: "Position X" })).toBeTruthy();
    expect(screen.getByRole("spinbutton", { name: "Scale Y" })).toBeTruthy();
    expect(screen.getByRole("spinbutton", { name: "Rotation" })).toBeTruthy();
    expect(screen.getByRole("spinbutton", { name: "Left" })).toBeTruthy();
    expect(screen.getByRole("slider", { name: "Opacity" })).toBeTruthy();
    expect(screen.getByRole("switch", { name: "Horizontal" })).toBeTruthy();
    expect(screen.getByLabelText("Blend mode")).toBeTruthy();
    // The keyframe diamonds, with the playhead inside the clip (1 s to 5 s).
    expect(screen.getByRole("button", { name: "Add keyframe for Position" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /keyframe for Blend mode/ })).toBeNull();
    // Clip info and Track.
    expect(screen.getByTestId("clip-source").textContent).toBe("beach.mp4");
    expect(screen.getByTestId("track-inspector")).toBeTruthy();
  });

  it("edits two clips with one command, and a typed +10 adds to the value", async () => {
    select("itm_v", "itm_a");
    // Two clips of their own: a linked pair inspects as the clip clicked (WP-2A.10).
    docStore.setState({ project: sampleProject({ itm_a: { linked_group: null } }) } as never);
    view();
    const user = userEvent.setup();
    const rotation = screen.getByRole("spinbutton", { name: "Rotation" });
    await user.dblClick(rotation);
    await user.keyboard("+10{Enter}");
    await waitFor(() => expect(calls("item.set_props")).toHaveLength(1));
    const [input] = calls("item.set_props") as Array<{
      items: string[];
      props: { transform: { rotation: number; position: null } };
      at: number;
    }>;
    expect(input?.items).toEqual(["itm_v", "itm_a"]);
    expect(input?.props.transform.rotation).toBe(10);
    expect(input?.props.transform.position).toBeNull();
    expect(input?.at).toBe(sec(2));
    // One batch transaction: one undo entry.
    expect(calls("edit.begin_tx")).toEqual([{ kind: "batch", label: "Change rotation" }]);
    expect(calls("edit.commit_tx")).toHaveLength(1);
  });

  it("keys a property at the playhead from the diamond", async () => {
    select("itm_v");
    view();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Add keyframe for Position" }));
    await waitFor(() => expect(calls("keyframe.set")).toHaveLength(1));
    expect(calls("keyframe.set")[0]).toMatchObject({
      items: ["itm_v"],
      prop: "transform.position",
      keys: [{ at: sec(2), t: null, value: null }],
    });
  });

  it("shows an animated value and removes the key the playhead sits on", async () => {
    const keyed = {
      visual: {
        transform: {
          position: {
            keyframes: [
              { t: sec(1), value: { x: 5, y: 6 }, interp: { kind: "linear" } },
              { t: sec(3), value: { x: 50, y: 60 }, interp: { kind: "linear" } },
            ],
          },
          scale: { x: 1, y: 1 },
          rotation: 0,
          anchor: { x: 0, y: 0 },
        },
        opacity: 1,
      },
    };
    docStore.setState({ project: sampleProject({ itm_v: keyed }), status: "ready" } as never);
    viewStore.setState({
      selection: [{ kind: "item", id: "itm_v" }],
      playhead: sec(2),
    } as never);
    call.mockImplementation(async (name: string) => {
      if (name === "edit.begin_tx") return { tx: "tx_1" };
      if (name === "media.proxy_status") return { proxies: [] };
      if (name === "keyframe.eval") {
        return { samples: [{ at: sec(2), t: sec(1), value: { x: 5, y: 6 }, in_item: true }] };
      }
      return {};
    });
    view();
    // The key at item time 1 s sits at sequence time 2 s: the diamond is on.
    const on = await screen.findByRole("button", {
      name: "Remove keyframe at playhead for Position",
    });
    expect(on.getAttribute("aria-pressed")).toBe("true");
    await waitFor(() =>
      expect(
        within(screen.getByRole("group", { name: "Transform" }))
          .getByRole("spinbutton", {
            name: "Position X",
          })
          .getAttribute("aria-valuenow"),
      ).toBe("5"),
    );
    await act(async () => {
      on.click();
    });
    await waitFor(() => expect(calls("keyframe.delete")).toHaveLength(1));
    expect(calls("keyframe.delete")[0]).toEqual({
      item: "itm_v",
      keys: [{ prop: "transform.position", t: sec(1) }],
    });
  });

  it("gives a property the schema adds a control, with no UI code change", () => {
    const set = original.find((c) => c.name === "item.set_props");
    if (set === undefined) throw new Error("item.set_props is not in the catalog");
    const props = set.inputSchema.properties?.props;
    commandCatalog.getState().register({
      ...set,
      inputSchema: {
        ...set.inputSchema,
        properties: {
          ...set.inputSchema.properties,
          props: {
            ...props,
            properties: {
              ...props?.properties,
              sharpness: {
                type: ["number", "null"],
                minimum: 0,
                maximum: 10,
                description: "Sharpness.",
              },
            },
          },
        },
      } as never,
    });
    select("itm_v");
    view();
    expect(screen.getByRole("spinbutton", { name: "Sharpness" })).toBeTruthy();
  });
});
