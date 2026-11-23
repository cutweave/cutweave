// SPDX-License-Identifier: GPL-3.0-or-later
// The media bin's pure parts (WP-1M.03): path validation, the analysis store, the drag store,
// duration labels and the bridge lookup.
import type { MediaId } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { mediaFilesBridge, pickedPaths } from "./bridge.ts";
import { formatDuration } from "./cards.ts";
import { createDragStore } from "./dragStore.ts";
import { MAX_IMPORT_PATHS, MAX_PATH_LENGTH, validateImportPaths } from "./import.ts";
import { createMediaStore } from "./mediaStore.ts";
import { placeholderThumbnails } from "./thumbnails.ts";

const F = 705_600_000;

describe("validateImportPaths", () => {
  it("keeps plain, distinct, bounded paths in order", () => {
    expect(validateImportPaths(["/a.mp4", "/b.mov", "/a.mp4"])).toEqual(["/a.mp4", "/b.mov"]);
  });

  it("drops everything that cannot be a path", () => {
    expect(
      validateImportPaths([
        "",
        5,
        null,
        undefined,
        {},
        "/nul\u0000.mp4",
        "x".repeat(MAX_PATH_LENGTH + 1),
      ]),
    ).toEqual([]);
    expect(validateImportPaths(["x".repeat(MAX_PATH_LENGTH)])).toHaveLength(1);
  });

  it("does not cap the list: the import sends it in engine-sized calls", () => {
    const many = Array.from({ length: MAX_IMPORT_PATHS + 5 }, (_, i) => `/f/${i}.mp4`);
    expect(validateImportPaths(many)).toHaveLength(MAX_IMPORT_PATHS + 5);
  });
});

describe("mediaStore", () => {
  const id = "med_a" as MediaId;

  it("records the latest failure of each stage and clears them when the asset is indexed", () => {
    const store = createMediaStore();
    const failed = (stage: string, error: string) =>
      store
        .getState()
        .applyAnalysisFailed({ media_id: id, stage, job_id: "job_1" as never, error });
    failed("thumbnails", "no decoder");
    failed("waveform", "bad audio");
    failed("thumbnails", "still no decoder");
    expect(store.getState().failures[id]).toEqual([
      { stage: "waveform", error: "bad audio" },
      { stage: "thumbnails", error: "still no decoder" },
    ]);
    store.getState().applyIndexed({ media_id: id });
    expect(store.getState().failures[id]).toBeUndefined();
    expect(store.getState().indexed[id]).toBe(1);
    store.getState().applyIndexed({ media_id: id });
    expect(store.getState().indexed[id]).toBe(2);
    store.getState().reset();
    expect(store.getState().indexed).toEqual({});
  });
});

describe("dragStore", () => {
  it("holds the dragged card until the drag ends", () => {
    const store = createDragStore();
    expect(store.getState().session).toBeNull();
    store.getState().begin({ id: "med_a", kind: "video", duration: F });
    expect(store.getState().session?.id).toBe("med_a");
    store.getState().end();
    expect(store.getState().session).toBeNull();
  });
});

describe("formatDuration", () => {
  it("shows m:ss, then h:mm:ss", () => {
    expect(formatDuration(0)).toBe("0:00");
    expect(formatDuration(9.6 * F)).toBe("0:10");
    expect(formatDuration(95 * F)).toBe("1:35");
    expect(formatDuration(3_725 * F)).toBe("1:02:05");
    expect(formatDuration(-5)).toBe("0:00");
  });
});

describe("mediaFilesBridge", () => {
  it("is empty without a desktop preload", () => {
    expect(mediaFilesBridge({})).toEqual({});
  });

  it("maps dropped files one by one and re-validates what comes back", () => {
    const bridge = mediaFilesBridge({
      cutweave: {
        files: { pathForFile: (f) => (f.name === "none.mp4" ? "" : `/drop/${f.name}`) },
        dialog: { pickMediaFiles: async () => ["/x.mp4"] },
      },
    });
    const files = ["a.mp4", "none.mp4", "a.mp4"].map((n) => new File(["x"], n));
    expect(bridge.pathsForDrop?.(files)).toEqual(["/drop/a.mp4"]);
    expect(bridge.pickMediaFiles).toBeTypeOf("function");
  });

  it("checks a picker answer once more on this side", () => {
    expect(pickedPaths(["/a.mp4", 3, ""])).toEqual(["/a.mp4"]);
    expect(pickedPaths(null)).toEqual([]);
    expect(pickedPaths("/a.mp4")).toEqual([]);
  });
});

describe("placeholderThumbnails", () => {
  it("always answers with the tile", () => {
    expect(
      placeholderThumbnails.thumbnailFor({
        id: "med_a" as MediaId,
        kind: "video",
        width: 160,
        indexed: 3,
      }),
    ).toEqual({ kind: "placeholder" });
  });
});
