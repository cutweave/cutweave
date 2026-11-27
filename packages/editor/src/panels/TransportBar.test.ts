// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// `<TransportBar>` and the transport keys (WP-1M.04): buttons and keys send `view.*` commands,
// the timecode follows the playhead (drop-frame at 29.97), and the controls stay off without an
// engine, with an empty timeline or for an engine without a transport.
import { MockTransport, type Project, RpcClient, type SequenceId } from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { preferences, prefsStore } from "../prefs/prefsStore.ts";
import { docStore } from "../state/docStore.ts";
import { initialPlaybackState, playbackStore } from "../state/playbackStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { FLICKS, sampleProject } from "../testing/fixtures.ts";
import { useTransportWiring } from "../transport/useTransport.ts";
import { TransportBar } from "./TransportBar.tsx";

function Wired() {
  useTransportWiring();
  return h(TransportBar);
}

const renderBar = () => render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(Wired))));

const state = (over: object = {}) => ({
  state: "stopped",
  playing: false,
  rate: 0,
  t: 0,
  rev: 1,
  quality: "full",
  dropped: 0,
  ...over,
});

let mock: MockTransport;

beforeAll(() => {
  // The volume slider (Radix) measures its thumb with ResizeObserver; jsdom has none.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

beforeEach(() => {
  viewStore.setState({ ...initialViewState, layouts: {} });
  playbackStore.setState({ ...initialPlaybackState });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  mock = new MockTransport();
  engineStore.setState({ rpc: null, connection: "none" });
});
afterEach(cleanup);

const connect = (project: Project = sampleProject()) => {
  engineStore.setState({ rpc: new RpcClient(mock), connection: "ready" });
  docStore.getState().resync({ rev: 1, committed_rev: 1, project });
};

describe("TransportBar", () => {
  it("is off without an engine and shows zero timecode", () => {
    renderBar();
    expect((screen.getByRole("button", { name: "Play" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByLabelText("Current time").textContent).toBe("00:00:00:00");
  });

  it("is off for an empty timeline", () => {
    const empty = sampleProject();
    for (const track of Object.values(empty.sequences["seq_1" as SequenceId]?.tracks ?? {})) {
      (track as { items: object }).items = {};
    }
    connect(empty);
    renderBar();
    expect((screen.getByRole("button", { name: "Play" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("play, step and go to start/end send view.* commands; the play button becomes pause", async () => {
    mock.handle("view.play", () => state({ state: "playing", playing: true, rate: 1 }));
    mock.handle("view.step", () => state({ t: FLICKS / 30 }));
    mock.handle("view.seek", (p) => state({ t: (p as { t: number }).t }));
    connect();
    renderBar();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Play" }));
    });
    expect(screen.getByRole("button", { name: "Pause" })).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Step forward one frame" }));
      fireEvent.click(screen.getByRole("button", { name: "Go to end" }));
    });
    expect(mock.requests.map((r) => [r.method, r.params])).toEqual([
      ["view.play", {}],
      ["view.step", { frames: 1 }],
      ["view.seek", { t: 5 * FLICKS - FLICKS / 30 }],
    ]);
  });

  it("Space, J/K/L and the arrows reach the transport through the keymap", async () => {
    for (const m of ["view.play", "view.pause", "view.set_rate", "view.step"]) {
      mock.handle(m, () => state());
    }
    connect();
    renderBar();
    await act(async () => {
      for (const key of [" ", "l", "j", "k", "ArrowRight", "ArrowLeft"]) {
        fireEvent.keyDown(document.body, { key });
      }
    });
    expect(mock.requests.map((r) => [r.method, r.params])).toEqual([
      ["view.play", {}],
      ["view.set_rate", { rate: 1 }],
      ["view.set_rate", { rate: -1 }],
      ["view.pause", {}],
      ["view.step", { frames: 1 }],
      ["view.step", { frames: -1 }],
    ]);
  });

  it("the timecode follows playback.state, drop-frame at 29.97", () => {
    const project = sampleProject();
    const seq = project.sequences["seq_1" as SequenceId] as unknown as {
      format: { frame_rate: object };
    };
    seq.format.frame_rate = { num: 30000, den: 1001 };
    connect(project);
    renderBar();
    act(() => {
      // One minute and two frames of 29.97: drop-frame skips ;00 and ;01.
      playbackStore.getState().applyState(state({ t: Math.round((1800 * FLICKS * 1001) / 30000) }));
    });
    expect(screen.getByLabelText("Current time").textContent).toBe("00:01:00;02");
  });

  it("an engine without transport commands turns playback off and says so once", async () => {
    connect();
    renderBar();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Play" }));
    });
    expect(playbackStore.getState().transport).toBe("unsupported");
    expect((screen.getByRole("button", { name: "Play" }) as HTMLButtonElement).disabled).toBe(true);
    warn.mockRestore();
  });

  it("the chip says what the frame was decoded from and the toggle keeps the proxy preference (WP-2F.02)", async () => {
    connect();
    renderBar();
    // What the engine reports: nothing from a proxy yet.
    expect(screen.getByLabelText("Preview source: Original").textContent).toBe("Original");
    act(() => playbackStore.getState().applyState(state({ use_proxies: true, proxy: true })));
    const chip = screen.getByLabelText("Preview source: Proxy");
    expect(chip.textContent).toBe("Proxy");
    expect(chip.getAttribute("data-source")).toBe("proxy");
    // The toggle is the preference `playback.proxies` (on by default); the runtime sends it on.
    const toggle = () => screen.getByRole("button", { name: "Use proxies in the preview" });
    expect(preferences.get("playback.proxies")).toBe(true);
    expect(toggle().getAttribute("aria-pressed")).toBe("true");
    await act(async () => {
      fireEvent.click(toggle());
      await Promise.resolve();
    });
    expect(preferences.get("playback.proxies")).toBe(false);
    expect(toggle().getAttribute("aria-pressed")).toBe("false");
    // The engine answers with the original on screen.
    act(() => playbackStore.getState().applyState(state({ use_proxies: false, proxy: false })));
    expect(screen.getByLabelText("Preview source: Original")).toBeTruthy();
    expect(playbackStore.getState().useProxies).toBe(false);
    await act(async () => {
      fireEvent.click(toggle());
      await Promise.resolve();
    });
    expect(preferences.get("playback.proxies")).toBe(true);
    // The toggle works without an engine too: it is this computer's preference.
    expect((toggle() as HTMLButtonElement).disabled).toBe(false);
    prefsStore.setState({
      values: { ...prefsStore.getState().values, "playback.proxies": true },
    });
  });

  it("the loop button is the preference playback.loop, both ways (WP-2H.10)", async () => {
    connect();
    renderBar();
    const loop = () => screen.getByRole("button", { name: "Loop playback" });
    expect(preferences.get("playback.loop")).toBe(false);
    expect(loop().getAttribute("aria-pressed")).toBe("false");
    await act(async () => {
      fireEvent.click(loop());
      await Promise.resolve();
    });
    expect(preferences.get("playback.loop")).toBe(true);
    expect(loop().getAttribute("aria-pressed")).toBe("true");
    // Changed elsewhere (Settings > Playback): the button follows.
    await act(async () => {
      await preferences.set("playback.loop", false);
    });
    expect(loop().getAttribute("aria-pressed")).toBe("false");
    // It is the person's preference: there with no engine, and never a command of its own.
    expect((loop() as HTMLButtonElement).disabled).toBe(false);
    expect(mock.requests.filter((r) => r.method === "view.set_loop")).toEqual([]);
  });

  it("mute and volume set master_gain; the quality indicator shows the frame's quality", async () => {
    mock.handle("view.set", () => ({}));
    connect();
    renderBar();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Mute" }));
    });
    expect(screen.getByRole("button", { name: "Unmute" }).getAttribute("aria-pressed")).toBe(
      "true",
    );
    expect(mock.requests.at(-1)).toMatchObject({
      method: "view.set",
      params: { patch: { master_gain: 0 } },
    });
    expect(screen.getByLabelText("Preview quality: Full")).toBeTruthy();
    act(() => playbackStore.getState().applyState(state({ quality: "quarter" })));
    expect(screen.getByLabelText("Preview quality: Quarter")).toBeTruthy();
  });
});
