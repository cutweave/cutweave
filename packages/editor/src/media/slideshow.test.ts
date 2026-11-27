// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2F.05d (ADR-0035): the editor's side of "Import as slideshow…" and "Add frame to media
// bin". Each is one catalog command with the arguments the CLI and MCP would pass; the rest is
// what the person is told.
import type { RpcClient } from "@cutweave/protocol";
import { RpcError } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { createToastStore } from "../state/toastStore.ts";
import { translator } from "../timeline/errors.ts";
import {
  addFrameToBin,
  importSlideshow,
  parseSeconds,
  proposedSeconds,
  type SlideshowDeps,
} from "./slideshow.ts";

const F = 705_600_000;
const SEQ = `seq_${"0".repeat(31)}1`;
const MED = `med_${"0".repeat(31)}7`;

function setup(answer: (name: string, args: unknown) => unknown) {
  const call = vi.fn(async (name: string, args: unknown) => answer(name, args));
  const engine = createStore<EngineStoreState>(
    () => ({ rpc: { call } as unknown as RpcClient, connection: "ready" }) as EngineStoreState,
  );
  const toasts = createToastStore();
  const deps: SlideshowDeps = { engine, toasts, t: translator(i18n.t.bind(i18n)) };
  const notices = () => toasts.getState().notices.map((n) => [n.tone, n.title, n.message]);
  return { call, deps, engine, notices };
}

describe("seconds per picture", () => {
  it("reads what a person types and refuses what is not a length", () => {
    expect(parseSeconds("2")).toBe(2);
    expect(parseSeconds(" 2.5 ")).toBe(2.5);
    expect(parseSeconds("0,5")).toBe(0.5);
    for (const bad of ["", "0", "-1", "abc", "1e3", "2s", "3601"]) {
      expect(parseSeconds(bad), bad).toBeNull();
    }
  });

  it("proposes the project's still duration, or five seconds", () => {
    expect(proposedSeconds(3 * F)).toBe("3");
    expect(proposedSeconds(F / 2)).toBe("0.5");
    expect(proposedSeconds(null)).toBe("5");
    expect(proposedSeconds(undefined)).toBe("5");
  });
});

describe("Import as slideshow", () => {
  it("sends one media.import_images_as_slideshow with the length in flicks and the order", async () => {
    const s = setup(() => ({ clips: ["a", "b", "c"], failed: [] }));
    const clips = await importSlideshow(s.deps, {
      files: ["/p/b.png", "/p/a.png", "/p/c.png", "/p/a.png", 7],
      seconds: 2.5,
      order: "name_numeric",
      sequence: SEQ as never,
    });
    expect(clips).toBe(3);
    expect(s.call).toHaveBeenCalledTimes(1);
    expect(s.call).toHaveBeenCalledWith("media.import_images_as_slideshow", {
      files: ["/p/b.png", "/p/a.png", "/p/c.png"],
      per_image: 2.5 * F,
      order: "name_numeric",
      sequence: SEQ,
    });
    expect(s.notices()).toEqual([
      ["success", "Slideshow imported", "3 pictures are on the timeline."],
    ]);
  });

  it("leaves the sequence to the engine when the editor shows none", async () => {
    const s = setup(() => ({ clips: ["a", "b"], failed: [] }));
    await importSlideshow(s.deps, {
      files: ["/p/a.png", "/p/b.png"],
      seconds: 5,
      order: "given",
      sequence: null,
    });
    expect(s.call).toHaveBeenCalledWith("media.import_images_as_slideshow", {
      files: ["/p/a.png", "/p/b.png"],
      per_image: 5 * F,
      order: "given",
    });
  });

  it("names every file that was left out", async () => {
    const s = setup(() => ({
      clips: ["a", "b"],
      failed: [
        { path: "/p/clip.mp4", code: "unsupported", error: "not a still picture" },
        { path: "C:\\p\\gone.png", code: "not_found", error: "no such file" },
      ],
    }));
    await importSlideshow(s.deps, {
      files: ["/p/a.png", "/p/b.png", "/p/clip.mp4", "C:\\p\\gone.png"],
      seconds: 1,
      order: "given",
      sequence: null,
    });
    expect(s.notices()).toEqual([
      [
        "danger",
        "clip.mp4 not imported",
        "This file is not a still picture, so it was left out of the slideshow.",
      ],
      [
        "danger",
        "gone.png not imported",
        "The file is missing. Check that it still exists and try again.",
      ],
      ["success", "Slideshow imported", "2 pictures are on the timeline."],
    ]);
  });

  it("says why when the engine refuses, and sends nothing without an engine or files", async () => {
    const s = setup(() => {
      throw new RpcError("invalid", "the range is not free");
    });
    const request = { files: ["/p/a.png", "/p/b.png"], seconds: 1, order: "given", sequence: null };
    expect(await importSlideshow(s.deps, request as never)).toBeNull();
    expect(s.notices()[0]?.slice(0, 2)).toEqual(["danger", "The slideshow was not imported"]);

    s.call.mockClear();
    expect(await importSlideshow(s.deps, { ...request, files: [] } as never)).toBeNull();
    s.engine.setState({ rpc: null });
    expect(await importSlideshow(s.deps, request as never)).toBeNull();
    expect(s.call).not.toHaveBeenCalled();
  });
});

describe("Add frame to media bin", () => {
  it("sends one media.import_frame for the Preview's frame at the playhead", async () => {
    const s = setup(() => ({ media_id: MED, name: "Sequence 1 00:00:02:12", duplicate: false }));
    expect(await addFrameToBin(s.deps, { kind: "sequence", at: 2.4 * F })).toBe(MED);
    expect(s.call).toHaveBeenCalledTimes(1);
    expect(s.call).toHaveBeenCalledWith("media.import_frame", {
      source: "sequence",
      at: Math.round(2.4 * F),
    });
    expect(s.notices()).toEqual([
      ["success", "Frame added to the media bin", "Sequence 1 00:00:02:12 is in the media bin."],
    ]);
  });

  it("takes the Source view's frame from the media that plays there", async () => {
    const s = setup(() => ({ media_id: MED, name: "clip.mp4 00:00:01:00", duplicate: true }));
    await addFrameToBin(s.deps, { kind: "media", media: MED as never, at: F });
    expect(s.call).toHaveBeenCalledWith("media.import_frame", { source: { media: MED }, at: F });
    expect(s.notices()).toEqual([
      [
        "info",
        "This frame is already in the media bin",
        "clip.mp4 00:00:01:00 shows the same picture.",
      ],
    ]);
  });

  it("shows the engine's reason when the project was never saved", async () => {
    const s = setup(() => {
      throw new RpcError("invalid", "the project has never been saved; save the project first");
    });
    expect(await addFrameToBin(s.deps, { kind: "sequence", at: 0 })).toBeNull();
    const [tone, title, message] = s.notices()[0] ?? [];
    expect([tone, title]).toEqual(["danger", "The frame was not added"]);
    expect(message).toContain("save the project first");
  });
});
