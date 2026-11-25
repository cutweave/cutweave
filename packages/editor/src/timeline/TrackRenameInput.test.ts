// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The in-place track name input: Enter and blur commit through `track.set` (once), Esc and an
// unchanged or blank name send nothing, and typing keeps the keys away from the timeline.
import { MockTransport, type Project, RpcClient } from "@cutweave/protocol";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createEngineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { createDocStore } from "../state/docStore.ts";
import { createToastStore } from "../state/toastStore.ts";
import { createViewStore } from "../state/viewStore.ts";
import { TimelineController } from "./controller.ts";
import { translator } from "./errors.ts";
import { TrackRenameInput } from "./TrackRenameInput.tsx";

const PROJECT = {
  meta: { name: "P" },
  settings: {},
  media: {},
  sequences: {
    seq_1: {
      id: "seq_1",
      name: "Main",
      format: { width: 1920, height: 1080, frame_rate: { num: 30, den: 1 } },
      track_order: ["trk_v"],
      tracks: {
        trk_v: {
          id: "trk_v",
          kind: "video",
          name: "V1",
          items: {},
          muted: false,
          hidden: false,
          locked: false,
          solo: false,
          magnetic: false,
          gain: 0,
          custom: {},
        },
      },
      custom: {},
    },
  },
  custom: {},
} as unknown as Project;

function setup() {
  const transport = new MockTransport().handle("track.set", () => ({}));
  const doc = createDocStore();
  doc.getState().resync({ rev: 1, committed_rev: 1, project: PROJECT });
  const engine = createEngineStore();
  engine.setState({ rpc: new RpcClient(transport), connection: "ready" });
  const toasts = createToastStore();
  const controller = new TimelineController({
    doc,
    view: createViewStore(),
    engine,
    toasts,
    t: translator(i18n.t.bind(i18n)),
  });
  controller.attach({
    getViewState: () => ({ pxPerSec: 100, scrollX: 0, scrollY: 0 }),
    itemsInRect: () => [],
    hitTest: () => {
      throw new Error("unused");
    },
    trackNameRect: () => ({ x: 40, y: 28, w: 100, h: 56 }),
  });
  return { transport, controller, toasts, doc };
}

// The input reads the doc through the module stores; point them at this test's doc.
let current: ReturnType<typeof setup>;
vi.mock("../state/docStore.ts", async (importOriginal) => {
  const real = await importOriginal<typeof import("../state/docStore.ts")>();
  return { ...real, useDocStore: (sel: (s: unknown) => unknown) => sel(current.doc.getState()) };
});

const sets = () => current.transport.requests.filter((r) => r.method === "track.set");
const mount = () => {
  render(h(I18nextProvider, { i18n }, h(TrackRenameInput, { controller: current.controller })));
};

beforeEach(() => {
  current = setup();
  current.controller.beginRename("trk_v");
});
afterEach(cleanup);

describe("TrackRenameInput", () => {
  it("opens focused over the name with the old name selected", () => {
    mount();
    const field = screen.getByRole("textbox", { name: "Track name" }) as HTMLInputElement;
    expect(field.value).toBe("V1");
    expect(document.activeElement).toBe(field);
    expect(field.selectionStart).toBe(0);
    expect(field.selectionEnd).toBe(2);
  });

  it("Enter commits the trimmed name through track.set exactly once", async () => {
    mount();
    const user = userEvent.setup();
    await user.keyboard("  B-roll ");
    await user.keyboard("{Enter}");
    await current.controller.settled();
    expect(sets()).toHaveLength(1);
    expect(sets()[0]?.params).toEqual({ track: "trk_v", name: "B-roll" });
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("blur commits, once", async () => {
    mount();
    const user = userEvent.setup();
    await user.keyboard("Music");
    fireEvent.blur(screen.getByRole("textbox"));
    expect(sets()).toHaveLength(1);
    expect(sets()[0]?.params).toEqual({ track: "trk_v", name: "Music" });
  });

  it("Esc cancels without a call, and the blur that follows sends nothing", async () => {
    mount();
    const user = userEvent.setup();
    await user.keyboard("Nope");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(sets()).toHaveLength(0);
  });

  it("a blank name on Enter is refused: no call, the reason shows and the old name returns", async () => {
    mount();
    const user = userEvent.setup();
    await user.keyboard("   {Enter}");
    expect(sets()).toHaveLength(0);
    const field = screen.getByRole("textbox") as HTMLInputElement;
    expect(field.value).toBe("V1");
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByRole("alert").textContent).toBe("A track needs a name. It stays V1.");
    expect(field.title).toBe("A track needs a name. It stays V1.");
  });

  it("typing keeps timeline keys away: S, Delete and F2 stop at the field", async () => {
    const outer = vi.fn();
    document.addEventListener("keydown", outer);
    mount();
    const user = userEvent.setup();
    await user.keyboard("s{Delete}{F2}");
    document.removeEventListener("keydown", outer);
    expect(outer).not.toHaveBeenCalled();
    expect((screen.getByRole("textbox") as HTMLInputElement).value).toContain("s");
  });
});
