// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-1M.14 component tests: hover-scrub on a bin card (the position line, the time, the one
// sprite request, nothing after the pointer leaves, the setting) and the Inspector's picture with
// its scrub bar (a slider in timecode, the keys, the newest frame only). The engine is a fake
// `ScrubSource` here; the same pictures come from a real engine in F31.
import { PREFERENCES } from "@cutweave/preferences";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { InspectorPanel } from "../panels/InspectorPanel.tsx";
import { MediaPanel } from "../panels/MediaPanel.tsx";
import { prefsStore } from "../prefs/prefsStore.ts";
import { docStore } from "../state/docStore.ts";
import { initialPlaybackState, playbackStore } from "../state/playbackStore.ts";
import { sourceViewStore } from "../state/sourceViewStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { createFakeEngine, type FakeEngine } from "../testing/fakeEngine.ts";
import { FLICKS, sampleProject } from "../testing/fixtures.ts";

import { type ScrubSource, setScrubSource } from "./scrubSource.ts";
import { placeholderThumbnails, setThumbnailSource } from "./thumbnails.ts";

void engineStore;

interface Fake {
  readonly source: ScrubSource;
  readonly sprite: ReturnType<typeof vi.fn>;
  readonly frame: ReturnType<typeof vi.fn>;
  readonly waveform: ReturnType<typeof vi.fn>;
}

function fakeSource(): Fake {
  const bitmap = () => ({ width: 160, height: 90, close: () => undefined });
  const sprite = vi.fn(async () => ({
    bitmap: bitmap(),
    manifest: {
      tile_w: 160,
      tile_h: 90,
      count: 100,
      columns: 10,
      start: 0,
      interval: 70_560_000,
    },
  }));
  const frame = vi.fn(async () => bitmap());
  const waveform = vi.fn(async () => bitmap());
  const source = { sprite, frame, waveform, dispose: () => undefined } as unknown as ScrubSource;
  return { source, sprite, frame, waveform };
}

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
  // jsdom has no canvas: drawing is a no-op, and the bar and the line are what is tested.
  HTMLCanvasElement.prototype.getContext = (() => null) as never;
});

let engine: FakeEngine;

function load(extra: object = {}) {
  const project = sampleProject();
  const base = (project.media as Record<string, object>).med_a as object;
  const media = {
    ...project.media,
    med_a: { ...base, content_hash: "blake3-fast:aa", ...extra },
    med_s: {
      ...base,
      id: "med_s",
      kind: "audio",
      content_hash: "blake3-fast:bb",
      probe: {
        streams: [{ index: 0, kind: "audio", codec: "pcm_s16le" }],
        duration: 4 * FLICKS,
        fps: null,
        vfr: false,
        rotation: 0,
        color: {},
        audio: { rate: 48000, channels: 2 },
        stub: false,
      },
    },
  };
  docStore.getState().resync({
    rev: 1,
    committed_rev: 1,
    project: { ...project, media },
  } as never);
}

const wrap = (child: ReturnType<typeof h>) =>
  h(I18nextProvider, { i18n }, h(TooltipProvider, null, child));

beforeEach(() => {
  viewStore.setState({ ...initialViewState, layouts: {} });
  prefsStore.setState({
    values: { ...prefsStore.getState().values, "media.hover_scrub": true },
  });
  engine = createFakeEngine();
  engine.install();
  load();
});

afterEach(() => {
  cleanup();
  engine.uninstall();
  setScrubSource(null);
  setThumbnailSource(placeholderThumbnails);
});

/** The first animation frame after now (the scrub asks once per frame). */
const nextFrame = () => act(() => new Promise<void>((done) => requestAnimationFrame(() => done())));

describe("hover-scrub on a card", () => {
  function setup() {
    const fake = fakeSource();
    setScrubSource(fake.source);
    const view = render(wrap(h(MediaPanel)));
    const card = view.container.querySelector('[data-media-id="med_a"]') as HTMLElement;
    const thumb = card.querySelector(".cw-media__thumb") as HTMLElement;
    thumb.getBoundingClientRect = () =>
      ({ left: 100, width: 200, top: 0, height: 112, right: 300, bottom: 112 }) as DOMRect;
    const move = (x: number) =>
      fireEvent.pointerMove(thumb, { clientX: x, pointerType: "mouse", buttons: 0 });
    return { ...fake, view, card, thumb, move };
  }

  it("shows the position line and the time, and asks for one sprite", async () => {
    const { thumb, move, sprite } = setup();
    expect(thumb.dataset.scrubbing).toBeUndefined();
    move(200);
    await nextFrame();
    expect(thumb.dataset.scrubbing).toBe("true");
    const line = thumb.querySelector(".cw-media__scrub-line") as HTMLElement;
    expect(line.style.insetInlineStart).toBe("50%");
    // Half way into 10 s at 30 fps: 5 seconds.
    expect(thumb.querySelector(".cw-media__scrub-time")?.textContent).toBe("00:00:05:00");
    for (const x of [110, 150, 200, 250, 290]) move(x);
    await nextFrame();
    expect(sprite).toHaveBeenCalledWith("med_a");
    expect(sprite.mock.calls.length).toBeLessThanOrEqual(2);
  });

  it("goes back to the poster when the pointer leaves, and asks nothing more", async () => {
    const { thumb, move, sprite } = setup();
    move(150);
    await nextFrame();
    const asked = sprite.mock.calls.length;
    fireEvent.pointerLeave(thumb);
    expect(thumb.dataset.scrubbing).toBeUndefined();
    expect((thumb.querySelector("canvas") as HTMLCanvasElement).hidden).toBe(true);
    await nextFrame();
    await nextFrame();
    expect(sprite.mock.calls.length).toBe(asked);
  });

  it("shows only the line and the time over a sound's waveform", async () => {
    const fake = fakeSource();
    setScrubSource(fake.source);
    const { container } = render(wrap(h(MediaPanel)));
    // The sound is an asset of the project too.
    const card = container.querySelector('[data-media-id="med_s"]') as HTMLElement;
    const thumb = card.querySelector(".cw-media__thumb") as HTMLElement;
    thumb.getBoundingClientRect = () => ({ left: 0, width: 100 }) as DOMRect;
    fireEvent.pointerMove(thumb, { clientX: 25, pointerType: "mouse", buttons: 0 });
    expect(thumb.dataset.scrubbing).toBe("true");
    expect(thumb.querySelector(".cw-media__scrub-time")?.textContent).toBe("00:00:01:00");
    await waitFor(() => expect(fake.waveform).toHaveBeenCalledTimes(1));
    expect(fake.sprite).not.toHaveBeenCalled();
  });

  it("does nothing with the setting off, for touch, or without the engine's media", async () => {
    const { thumb, move, sprite, view } = setup();
    prefsStore.setState({
      values: { ...prefsStore.getState().values, "media.hover_scrub": false },
    });
    await nextFrame();
    expect(thumb.querySelector("canvas")).toBeNull();
    fireEvent.pointerMove(thumb, { clientX: 150, pointerType: "mouse", buttons: 0 });
    await nextFrame();
    expect(sprite).not.toHaveBeenCalled();
    prefsStore.setState({ values: { ...prefsStore.getState().values, "media.hover_scrub": true } });
    await nextFrame();
    const again = view.container.querySelector(".cw-media__thumb") as HTMLElement;
    again.getBoundingClientRect = () => ({ left: 0, width: 100 }) as DOMRect;
    fireEvent.pointerMove(again, { clientX: 50, pointerType: "touch", buttons: 0 });
    await nextFrame();
    expect(sprite).not.toHaveBeenCalled();
    act(() => setScrubSource(null));
    move(150);
    expect(thumb.dataset.scrubbing).toBeUndefined();
  });

  it("keeps the PREFERENCES default on", () => {
    expect(PREFERENCES["media.hover_scrub"]).toMatchObject({ type: "boolean", default: true });
  });
});

describe("the Inspector's picture", () => {
  function select(id: string) {
    viewStore.setState({ selection: [{ kind: "media", id } as never] });
  }

  it("has a slider in timecode that steps by frame and jumps to the ends", async () => {
    const fake = fakeSource();
    setScrubSource(fake.source);
    select("med_a");
    render(wrap(h(InspectorPanel)));
    const bar = screen.getByRole("slider", { name: "Scrub position" });
    // 10 s at 30 fps.
    expect(bar.getAttribute("aria-valuemin")).toBe("0");
    expect(bar.getAttribute("aria-valuemax")).toBe("299");
    expect(bar.getAttribute("aria-valuenow")).toBe("0");
    expect(bar.getAttribute("aria-valuetext")).toBe("00:00:00:00");
    fireEvent.keyDown(bar, { key: "ArrowRight" });
    fireEvent.keyDown(bar, { key: "ArrowRight" });
    expect(bar.getAttribute("aria-valuetext")).toBe("00:00:00:02");
    fireEvent.keyDown(bar, { key: "ArrowLeft" });
    expect(bar.getAttribute("aria-valuenow")).toBe("1");
    fireEvent.keyDown(bar, { key: "PageUp" });
    expect(bar.getAttribute("aria-valuetext")).toBe("00:00:01:01");
    fireEvent.keyDown(bar, { key: "End" });
    expect(bar.getAttribute("aria-valuetext")).toBe("00:00:09:29");
    fireEvent.keyDown(bar, { key: "ArrowRight" });
    expect(bar.getAttribute("aria-valuenow")).toBe("299");
    fireEvent.keyDown(bar, { key: "Home" });
    expect(bar.getAttribute("aria-valuetext")).toBe("00:00:00:00");
    expect(screen.getByTestId("media-preview-time").textContent).toBe("00:00:00:00");
  });

  it("asks for the frame under the bar, at most once per animation frame", async () => {
    const fake = fakeSource();
    setScrubSource(fake.source);
    select("med_a");
    render(wrap(h(InspectorPanel)));
    const bar = screen.getByRole("slider");
    await nextFrame();
    expect(fake.frame).toHaveBeenCalledTimes(1);
    expect(fake.frame.mock.calls[0]?.[1]).toBe(0);
    for (let i = 0; i < 5; i++) fireEvent.keyDown(bar, { key: "ArrowRight" });
    await nextFrame();
    await nextFrame();
    expect(fake.frame).toHaveBeenCalledTimes(2);
    // Frame 5 of 30 fps, in flicks.
    expect(fake.frame.mock.calls[1]?.[1]).toBe(5 * (705_600_000 / 30));
  });

  it("maps a click along the bar to the frame at that fraction", () => {
    setScrubSource(fakeSource().source);
    select("med_a");
    render(wrap(h(InspectorPanel)));
    const bar = screen.getByRole("slider");
    bar.getBoundingClientRect = () => ({ left: 0, width: 300 }) as DOMRect;
    fireEvent.pointerDown(bar, { clientX: 150, button: 0, pointerId: 1 });
    // Half of 300 frames.
    expect(bar.getAttribute("aria-valuenow")).toBe("150");
    expect(bar.getAttribute("aria-valuetext")).toBe("00:00:05:00");
    fireEvent.pointerMove(bar, { clientX: 300, pointerId: 1 });
    expect(bar.getAttribute("aria-valuenow")).toBe("299");
    fireEvent.pointerUp(bar, { pointerId: 1 });
    fireEvent.pointerMove(bar, { clientX: 0, pointerId: 1 });
    expect(bar.getAttribute("aria-valuenow")).toBe("299");
  });

  it("shows a sound's waveform with the same bar", async () => {
    const fake = fakeSource();
    setScrubSource(fake.source);
    select("med_s");
    render(wrap(h(InspectorPanel)));
    await waitFor(() => expect(fake.waveform).toHaveBeenCalledTimes(1));
    expect(fake.frame).not.toHaveBeenCalled();
    const bar = screen.getByRole("slider");
    // 4 s at the 30 fps timecode base of a sound.
    expect(bar.getAttribute("aria-valuemax")).toBe("119");
  });

  it("is absent without the engine's media, and for a clip the old Inspector stays", () => {
    select("med_a");
    render(wrap(h(InspectorPanel)));
    expect(screen.queryByTestId("media-preview")).toBeNull();
    expect(screen.getByTestId("media-inspector")).toBeTruthy();
  });

  it("has no axe violations", async () => {
    setScrubSource(fakeSource().source);
    select("med_a");
    const { container } = render(wrap(h(InspectorPanel)));
    expect((await axe.run(container)).violations).toEqual([]);
  });
});

describe("play in the Inspector (WP-1M.15)", () => {
  const SOURCE = { media: "med_a", duration: 10 * FLICKS, fps: { num: 30, den: 1 } };
  const answer = (over: object = {}) => ({
    state: "playing",
    playing: true,
    rate: 1,
    t: 0,
    quality: "full",
    dropped: 0,
    source: SOURCE,
    ...over,
  });

  function setup() {
    setScrubSource(fakeSource().source);
    viewStore.setState({ selection: [{ kind: "media", id: "med_a" } as never] });
    playbackStore.setState({ ...initialPlaybackState });
    sourceViewStore.setState({ host: "preview" });
    engine.answer("view.set_source", (input) =>
      (input as { media?: string }).media === undefined
        ? answer({ source: null, playing: false })
        : answer({ playing: false }),
    );
    engine.answer("view.play", () => answer());
    engine.answer("view.pause", () => answer({ playing: false, state: "stopped" }));
    engine.answer("view.seek", (input) => answer({ t: (input as { t: number }).t }));
    return render(wrap(h(InspectorPanel)));
  }

  it("plays the asset as the source from the bar's position, hosted by the Inspector", async () => {
    setup();
    fireEvent.keyDown(screen.getByRole("slider", { name: "Scrub position" }), { key: "PageUp" });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /^Play / }));
    });
    // The bar was at one second: the source starts there.
    expect(engine.calls.map((c) => [c.name, c.input])).toContainEqual([
      "view.seek",
      { t: 30 * (FLICKS / 30) },
    ]);
    expect(engine.names().filter((n) => n.startsWith("view."))).toEqual([
      "view.set_source",
      "view.seek",
      "view.play",
    ]);
    expect(sourceViewStore.getState().host).toBe("inspector");
    expect(screen.getByRole("button", { name: /^Pause / })).toBeTruthy();
  });

  it("the bar follows the source's playhead, and moving it seeks the source", async () => {
    setup();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /^Play / }));
    });
    act(() => playbackStore.getState().applyState(answer({ t: 3 * FLICKS })));
    const bar = screen.getByRole("slider", { name: "Scrub position" });
    expect(bar.getAttribute("aria-valuetext")).toBe("00:00:03:00");
    await act(async () => {
      fireEvent.keyDown(bar, { key: "ArrowRight" });
    });
    expect(engine.calls.at(-1)).toMatchObject({
      name: "view.seek",
      input: { t: 90 * (FLICKS / 30) + FLICKS / 30 },
    });
    // The timeline's playhead is not the source's.
    expect(viewStore.getState().playhead).toBe(0);
  });

  it("pauses what plays here, and ends the source when the Inspector leaves the asset", async () => {
    const view = setup();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /^Play / }));
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /^Pause / }));
    });
    expect(engine.calls.at(-1)?.name).toBe("view.pause");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /^Play / }));
    });
    await act(async () => {
      view.unmount();
    });
    expect(engine.calls.at(-1)).toMatchObject({ name: "view.set_source", input: {} });
    expect(playbackStore.getState().source).toBeNull();
  });

  it("a source the Preview shows is not ended when the Inspector leaves the asset", async () => {
    const view = setup();
    await act(async () => {
      playbackStore.getState().applyState(answer());
    });
    view.unmount();
    expect(engine.names().filter((n) => n.startsWith("view."))).toEqual([]);
  });
});
