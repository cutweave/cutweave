// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2C.05: the timeline toolbar during a clip drag. Every update of a drag patches the
// document; the toolbar (with its Keyframe lanes button) must not render again for it, and the
// button asks the host for one small value, which the host answers from memory.
import { act, cleanup, render, screen } from "@testing-library/react";
import { createElement as h, Profiler } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../i18n/index.ts";
import { docStore } from "../state/docStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { sampleProject } from "../testing/fixtures.ts";
import type { TimelineHost, TimelineUi } from "./host.ts";
import type { LanesToggle } from "./keyframes.ts";
import { TimelineToolbar } from "./TimelineToolbar.tsx";

afterEach(cleanup);

const UI: TimelineUi = {
  tool: "select",
  linkedSelection: true,
  snapGuides: [],
  marquee: null,
  preview: null,
  replaceTarget: null,
  cursor: "default",
  state: "moving",
};

function mount(state: { toggle: LanesToggle }) {
  const lanesToggleState = vi.fn(() => state.toggle);
  const host = { lanesToggleState, toggleKeyframeLanes: vi.fn() } as unknown as TimelineHost;
  let commits = 0;
  const tree = (ui: TimelineUi) =>
    h(
      I18nextProvider,
      { i18n },
      h(
        Profiler,
        { id: "toolbar", onRender: () => void (commits += 1) },
        h(TimelineToolbar, { host, ui, live: true }),
      ),
    );
  const view = render(tree(UI));
  return {
    lanesToggleState,
    commits: () => commits,
    /** The panel renders the toolbar again with a new `ui`, as every machine event makes it. */
    again: () => view.rerender(tree({ ...UI })),
  };
}

describe("TimelineToolbar during a drag", () => {
  it("does not render again when the document is patched and the toggle's value is the same", () => {
    const state = { toggle: "none" as LanesToggle };
    const { commits } = mount(state);
    const mounted = commits();
    // A hundred drag updates: a new document each time, as `doc.patched` makes.
    for (let update = 0; update < 100; update += 1) {
      act(() => docStore.setState({ project: { ...sampleProject() } }));
    }
    act(() => viewStore.getState().setPlayhead(123));
    expect(commits()).toBe(mounted);
  });

  it("a render of the toolbar around it does not render the Keyframe lanes button", () => {
    const state = { toggle: "closed" as LanesToggle };
    const { lanesToggleState, again, commits } = mount(state);
    const asked = lanesToggleState.mock.calls.length;
    const mounted = commits();
    // A hundred pointer moves of a drag: the panel hands the toolbar a new `ui` each time.
    for (let move = 0; move < 100; move += 1) again();
    expect(commits()).toBe(mounted + 100);
    // The button's render is the only thing that asks the host: it was never asked.
    expect(lanesToggleState.mock.calls.length).toBe(asked);
  });

  it("the Keyframe lanes button follows the host's answer", () => {
    const state = { toggle: "none" as LanesToggle };
    const { commits } = mount(state);
    const button = () => screen.getByRole("button", { name: /^Keyframe lanes/ });
    expect((button() as HTMLButtonElement).disabled).toBe(true);
    const before = commits();
    state.toggle = "closed";
    act(() => docStore.setState({ project: { ...sampleProject() } }));
    expect(commits()).toBe(before + 1);
    expect((button() as HTMLButtonElement).disabled).toBe(false);
    expect(button().getAttribute("aria-pressed")).toBe("false");
    state.toggle = "open";
    act(() => viewStore.getState().setKeyframeLanes(["itm_x"]));
    expect(button().getAttribute("aria-pressed")).toBe("true");
  });
});
