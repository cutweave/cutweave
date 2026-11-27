// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// `<TransportBar>` and the transport keys (WP-1M.04): buttons and keys send `view.*` commands,
// the timecode follows the playhead (drop-frame at 29.97), and the controls stay off without an
// engine, with an empty timeline or for an engine without a transport.
import { MockTransport, type Project, RpcClient, type SequenceId } from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

// #225: the bar measures itself and moves what does not fit into the More menu. jsdom lays
// nothing out, so the boxes are stood in for: the bar is `width` wide, the timecode takes 100 px
// and every control in the other two groups 40 px. That makes the levels need 580, 540, 460, 420,
// 380, 260 and 180 px.
describe("TransportBar in a narrow panel (#225)", () => {
  let width = 0;
  const resized = new Set<() => void>();

  beforeEach(() => {
    resized.clear();
    vi.stubGlobal(
      "ResizeObserver",
      class {
        readonly #callback: () => void;
        constructor(callback: () => void) {
          this.#callback = callback;
        }
        observe() {
          resized.add(this.#callback);
        }
        unobserve() {}
        disconnect() {
          resized.delete(this.#callback);
        }
      },
    );
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
      this: HTMLElement,
    ) {
      const controls = this.querySelectorAll("button, output, .cw-slider").length;
      const w = this.classList.contains("cw-transport")
        ? width
        : this.classList.contains("cw-transport__time")
          ? 100
          : controls * 40;
      return { x: 0, y: 0, left: 0, top: 0, right: w, bottom: 0, width: w, height: 0 } as DOMRect;
    });
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  /** The panel becomes `next` pixels wide. */
  const resize = (next: number) =>
    act(() => {
      width = next;
      for (const callback of [...resized]) callback();
    });

  const button = (name: string) => screen.queryByRole("button", { name });
  const more = () => screen.getByRole("button", { name: "More controls" });
  /** The open menu's rows (commands, choices and submenus), in order. */
  const rows = () => [...document.querySelectorAll('[role="menu"] [role^="menuitem"]')];
  /** Opens a submenu of the open menu from the keyboard: focus on its row, then Right. */
  const openSub = async (user: ReturnType<typeof userEvent.setup>, name: string) => {
    act(() => screen.getByRole("menuitem", { name }).focus());
    await user.keyboard("{ArrowRight}");
  };
  const openMore = async (user: ReturnType<typeof userEvent.setup>) => {
    await user.click(more());
    const menu = await screen.findByRole("menu", { name: "More transport controls" });
    // Open means the focus is in it (the keys go to the menu).
    await waitFor(() => expect(menu.contains(document.activeElement)).toBe(true));
    return menu;
  };

  it("with room for everything there is no More menu", () => {
    width = 600;
    connect();
    renderBar();
    expect(button("More controls")).toBeNull();
    for (const name of ["Go to start", "Loop playback", "Mute", "Full screen preview"]) {
      expect(button(name)).toBeTruthy();
    }
    expect(screen.getByRole("slider", { name: "Volume" })).toBeTruthy();
    expect(document.querySelector(".cw-transport")?.getAttribute("data-fit")).toBeNull();
  });

  it("full screen, the chip, the quality and the proxies toggle go first, and work from the menu", async () => {
    const user = userEvent.setup();
    width = 500;
    connect();
    renderBar();
    // In the bar: the playback buttons, loop, mute and the volume.
    for (const name of ["Go to start", "Play", "Go to end", "Loop playback", "Mute"]) {
      expect(button(name)).toBeTruthy();
    }
    expect(screen.getByRole("slider", { name: "Volume" })).toBeTruthy();
    // Not in the bar any more.
    expect(button("Full screen preview")).toBeNull();
    expect(button("Use proxies in the preview")).toBeNull();
    expect(screen.queryByLabelText("Preview source: Original")).toBeNull();
    expect(screen.queryByLabelText("Preview quality: Full")).toBeNull();

    const menu = await openMore(user);
    // The readouts are the menu's headings; full screen does nothing yet, as in the bar.
    expect(menu.textContent).toContain("Preview quality: Full");
    expect(menu.textContent).toContain("Preview source: Original");
    expect(
      screen.getByRole("menuitem", { name: "Full screen preview" }).getAttribute("aria-disabled"),
    ).toBe("true");
    const proxies = screen.getByRole("menuitemcheckbox", { name: "Use proxies in the preview" });
    expect(proxies.getAttribute("aria-checked")).toBe("true");
    expect(preferences.get("playback.proxies")).toBe(true);
    await user.click(proxies);
    expect(preferences.get("playback.proxies")).toBe(false);

    // The quality while playing: the same choices as the quality readout's own menu.
    await openMore(user);
    await openSub(user, "Quality while playing");
    const half = await screen.findByRole("menuitemradio", { name: "Half" });
    expect(screen.getByRole("menuitemradio", { name: "Auto" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    await user.click(half);
    await waitFor(() => expect(preferences.get("playback.quality_playing")).toBe("half"));
    await act(async () => {
      await preferences.set("playback.proxies", true);
      await preferences.set("playback.quality_playing", "auto");
    });
  });

  it("at the narrowest only play, the timecode and More stay; the rest sends the same commands from the menu", async () => {
    const user = userEvent.setup();
    mock.handle("view.step", () => state({ t: FLICKS / 30 }));
    mock.handle("view.seek", (p) => state({ t: (p as { t: number }).t }));
    mock.handle("view.set", () => ({}));
    width = 200;
    connect();
    renderBar();
    expect(screen.getAllByRole("button").map((b) => b.getAttribute("aria-label"))).toEqual([
      "Play",
      "More controls",
    ]);
    expect(screen.getByLabelText("Current time").textContent).toBe("00:00:00:00");
    expect(screen.queryByRole("slider")).toBeNull();
    expect(document.querySelector(".cw-transport")?.getAttribute("data-fit")).toBe("tight");

    await openMore(user);
    // The bar's order: navigation, then loop and sound, then the picture.
    expect(rows().map((row) => row.querySelector(".cw-menu__label")?.textContent)).toEqual([
      "Go to start",
      "Step back one frame",
      "Step forward one frame",
      "Go to end",
      "Loop playback",
      "Mute",
      "Volume",
      "Quality while playing",
      "Use proxies in the preview",
      "Full screen preview",
    ]);
    // Stepping keeps the menu open, to step again.
    const step = screen.getByRole("menuitem", { name: /^Step forward one frame/ });
    await user.click(step);
    await user.click(step);
    await user.click(screen.getByRole("menuitem", { name: "Go to end" }));
    expect(mock.requests.map((r) => [r.method, r.params])).toEqual([
      ["view.step", { frames: 1 }],
      ["view.step", { frames: 1 }],
      ["view.seek", { t: 5 * FLICKS - FLICKS / 30 }],
    ]);

    await openMore(user);
    await user.click(screen.getByRole("menuitemcheckbox", { name: "Loop playback" }));
    expect(preferences.get("playback.loop")).toBe(true);
    await openMore(user);
    expect(
      screen.getByRole("menuitemcheckbox", { name: "Loop playback" }).getAttribute("aria-checked"),
    ).toBe("true");
    await user.click(screen.getByRole("menuitemcheckbox", { name: "Mute" }));
    expect(mock.requests.at(-1)).toMatchObject({
      method: "view.set",
      params: { patch: { master_gain: 0 } },
    });
    expect(viewStore.getState().muted).toBe(true);
    await user.click(more());
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Mute" }));
    expect(viewStore.getState().muted).toBe(false);

    // The volume: a few fixed steps while the slider is away.
    await openMore(user);
    await openSub(user, "Volume");
    await user.click(await screen.findByRole("menuitemradio", { name: /^50\s%$/ }));
    expect(mock.requests.at(-1)).toMatchObject({
      method: "view.set",
      params: { patch: { master_gain: 0.5 } },
    });
    await act(async () => {
      await preferences.set("playback.loop", false);
    });
  });

  it("the menu's rows are off when the bar's buttons would be", async () => {
    const user = userEvent.setup();
    width = 200;
    renderBar();
    await openMore(user);
    const row = (label: string) =>
      rows().find((r) => r.querySelector(".cw-menu__label")?.textContent === label);
    for (const label of ["Step back one frame", "Go to end", "Mute", "Volume"]) {
      expect(row(label)?.getAttribute("aria-disabled")).toBe("true");
    }
    // The preferences are this computer's: there with no engine, as in the bar.
    expect(
      screen.getByRole("menuitemcheckbox", { name: "Loop playback" }).getAttribute("aria-disabled"),
    ).toBeNull();
  });

  it("controls leave one step at a time as the panel narrows and come back as it widens", () => {
    width = 600;
    connect();
    renderBar();
    const inBar = () => ({
      fullscreen: button("Full screen preview") !== null,
      proxies: button("Use proxies in the preview") !== null,
      volume: screen.queryByRole("slider") !== null,
      loop: button("Loop playback") !== null,
      mute: button("Mute") !== null,
      toEnd: button("Go to end") !== null,
      step: button("Step back one frame") !== null,
    });
    const all = {
      fullscreen: true,
      proxies: true,
      volume: true,
      loop: true,
      mute: true,
      toEnd: true,
      step: true,
    };
    expect(inBar()).toEqual(all);
    resize(560);
    expect(inBar()).toEqual({ ...all, fullscreen: false });
    resize(470);
    expect(inBar()).toEqual({ ...all, fullscreen: false, proxies: false });
    resize(430);
    expect(inBar()).toEqual({ ...all, fullscreen: false, proxies: false, volume: false });
    resize(390);
    expect(inBar()).toEqual({
      ...all,
      fullscreen: false,
      proxies: false,
      volume: false,
      loop: false,
    });
    resize(300);
    expect(inBar()).toEqual({
      fullscreen: false,
      proxies: false,
      volume: false,
      loop: false,
      mute: false,
      toEnd: false,
      step: true,
    });
    resize(190);
    expect(inBar().step).toBe(false);
    expect(button("Play")).toBeTruthy();
    // Wider again: straight back, as far as there is room.
    resize(480);
    expect(inBar()).toEqual({ ...all, fullscreen: false, proxies: false });
    resize(1000);
    expect(inBar()).toEqual(all);
    expect(button("More controls")).toBeNull();
  });

  it("a control that grows (the chip's text, a density) is measured again", () => {
    width = 585;
    connect();
    renderBar();
    expect(button("Full screen preview")).toBeTruthy();
    // Everything in the bar is now wider than the panel: the first step leaves.
    vi.mocked(HTMLElement.prototype.getBoundingClientRect).mockImplementation(function (
      this: HTMLElement,
    ) {
      const controls = this.querySelectorAll("button, output, .cw-slider").length;
      const w = this.classList.contains("cw-transport")
        ? width
        : this.classList.contains("cw-transport__time")
          ? 100
          : controls * 44;
      return { x: 0, y: 0, left: 0, top: 0, right: w, bottom: 0, width: w, height: 0 } as DOMRect;
    });
    resize(585);
    expect(button("Full screen preview")).toBeNull();
    expect(button("Use proxies in the preview")).toBeTruthy();
  });
});
