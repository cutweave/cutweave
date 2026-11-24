// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The preview panel's path choice (WP-1M.04): the GPU path when the desktop bridge can present
// the engine's ring, the JPEG fallback with a banner that says why otherwise, and the overlay.
// jsdom has no canvas: drawing itself is covered by the presenter tests and the Playwright e2e.
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, render, screen } from "@testing-library/react";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { docStore } from "../state/docStore.ts";
import { initialPlaybackState, playbackStore } from "../state/playbackStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { sampleProject } from "../testing/fixtures.ts";
import { PreviewPanel } from "./PreviewPanel.tsx";

const RING = { name: "cwp-00000001-0000000000000002", epoch: "0x3" };

const renderPanel = () =>
  render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(PreviewPanel))));

function desktop(over: { preview?: object; forced?: boolean } = {}) {
  (globalThis as unknown as { cutweave?: object }).cutweave = {
    app: { getInfo: async () => (over.forced ? { preview: "jpeg" } : {}) },
    engine: { getConnection: async () => null },
    ...(over.preview && { preview: over.preview }),
  };
}

function fakeBridge(attach: object = { ok: true, adapter: "Fake GPU", upload: "writeTexture" }) {
  return {
    isAvailable: () => true,
    attachCanvas: vi.fn(async () => attach),
    detachCanvas: vi.fn(),
    redraw: () => true,
    open: vi.fn(),
    startPresenting: vi.fn(),
    stopPresenting: vi.fn(),
    getStatus: () => ({ state: "live" }),
    getPresentStats: () => ({ running: true, errors: 0 }),
    close: vi.fn(),
  };
}

const surface = () => document.querySelector(".cw-preview__surface");

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
  docStore.getState().resync({ rev: 1, committed_rev: 1, project: sampleProject() });
  engineStore.setState({ rpc: null, connection: "ready", previewRing: RING });
});
afterEach(() => {
  cleanup();
  delete (globalThis as unknown as { cutweave?: object }).cutweave;
  engineStore.setState({ connection: "none", previewRing: undefined });
});

describe("PreviewPanel", () => {
  it("without an engine shows the empty frame and no canvas", () => {
    engineStore.setState({ connection: "none", previewRing: undefined });
    renderPanel();
    expect(screen.getByText("Nothing to preview")).toBeTruthy();
    expect(surface()).toBeNull();
  });

  it("presents through the desktop bridge when it can (no banner)", async () => {
    const bridge = fakeBridge();
    desktop({ preview: bridge });
    renderPanel();
    await vi.waitFor(() => expect(surface()?.getAttribute("data-live")).toBe("true"));
    expect(surface()?.getAttribute("data-path")).toBe("gpu");
    expect(bridge.open).toHaveBeenCalledWith(RING.name, RING.epoch);
    expect(bridge.startPresenting).toHaveBeenCalledWith({ latencyMs: 20 });
    expect(screen.queryByText("Reduced preview")).toBeNull();
    expect(screen.getByRole("img", { name: "Preview of the frame at the playhead" })).toBeTruthy();
  });

  it("falls back to still frames when WebGPU cannot present, and says so", async () => {
    desktop({ preview: fakeBridge({ ok: false, reason: "no-adapter", message: "none" }) });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    renderPanel();
    await vi.waitFor(() => expect(surface()?.getAttribute("data-path")).toBe("jpeg"));
    expect(surface()?.getAttribute("data-reason")).toBe("webgpu");
    expect(document.querySelector(".cw-preview__banner")?.textContent).toContain(
      "WebGPU is not available here",
    );
    warn.mockRestore();
  });

  it("falls back when the engine publishes no ring", async () => {
    engineStore.setState({ previewRing: null });
    desktop({ preview: fakeBridge() });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    renderPanel();
    await vi.waitFor(() => expect(surface()?.getAttribute("data-reason")).toBe("no-ring"));
    warn.mockRestore();
  });

  it("CUTWEAVE_PREVIEW=jpeg forces the fallback; no bridge means no-addon", async () => {
    desktop({ preview: fakeBridge(), forced: true });
    const first = renderPanel();
    await vi.waitFor(() => expect(surface()?.getAttribute("data-reason")).toBe("forced"));
    expect(document.querySelector(".cw-preview__banner")?.textContent).toContain(
      "CUTWEAVE_PREVIEW=jpeg",
    );
    first.unmount();
    desktop();
    renderPanel();
    await vi.waitFor(() => expect(surface()?.getAttribute("data-reason")).toBe("no-addon"));
  });

  it("the overlay shows safe areas when toggled and the selected clip's outline", async () => {
    desktop();
    renderPanel();
    expect(document.querySelectorAll(".cw-preview__safe")).toHaveLength(0);
    act(() => screen.getByRole("button", { name: "Safe areas" }).click());
    expect(document.querySelectorAll(".cw-preview__safe")).toHaveLength(2);
    act(() => viewStore.getState().setSelection([{ kind: "item", id: "itm_v" as never }]));
    const outline = document.querySelector<HTMLElement>(".cw-preview__selection");
    expect(outline?.style.width).toBe("100%");
    expect(document.querySelector(".cw-preview__readout")?.textContent).toBe("00:00:00:00");
  });

  it("the aspect switch shows the sequence's shape, not the workspace's, and is off", () => {
    desktop();
    viewStore.getState().setAspect("9:16");
    renderPanel();
    const checked = () =>
      screen
        .getAllByRole("radio")
        .filter((r) => r.getAttribute("aria-checked") === "true")
        .map((r) => r.textContent);
    // The fixture's sequence is 1920x1080.
    expect(checked()).toEqual(["16:9"]);
    expect(screen.getByRole("radio", { name: "16:9" }).hasAttribute("disabled")).toBe(true);
    // A shape no preset matches selects none.
    act(() => {
      const project = docStore.getState().project;
      if (project === null) throw new Error("no project");
      const seq = Object.values(project.sequences)[0];
      if (seq === undefined) throw new Error("no sequence");
      docStore.getState().resync({
        rev: 2,
        committed_rev: 2,
        project: {
          ...project,
          sequences: {
            ...project.sequences,
            [seq.id]: { ...seq, format: { ...seq.format, width: 1920, height: 800 } },
          },
        },
      });
    });
    expect(checked()).toEqual([]);
  });
});
