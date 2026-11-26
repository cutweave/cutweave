// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.09: the draw path makes no allocation per drawn item (no objects, arrays, closures or
// strings per clip, tile or label in a steady frame). Measured as the growth of V8's young
// generation across frames over the 5,000-clip project, with a context whose methods do nothing,
// so only the renderer's own allocations count; frames during which a collection ran are skipped.
// What remains is a small constant per frame (V8 boxing the large flick values passed between
// functions), the same with 75 clips on screen as with 2,600.
import { getHeapSpaceStatistics } from "node:v8";
import { describe, expect, it } from "vitest";
import { generateTimeline, withMarkers } from "../fixtures.ts";
import { fakeCanvas, testTheme } from "../test-support.ts";
import { FLICKS_PER_SECOND as F } from "../time.ts";
import { TimelineView } from "../timeline-view.ts";

/** A 2D context whose every method is one shared no-op (no Proxy, no closures per call). */
function noopContext(): CanvasRenderingContext2D {
  const noop = (): void => {};
  const metrics = { width: 0 };
  const ctx: Record<string, unknown> = {
    measureText: (s: string) => {
      metrics.width = s.length * 6;
      return metrics;
    },
  };
  for (const name of [
    "save",
    "restore",
    "setTransform",
    "beginPath",
    "closePath",
    "rect",
    "roundRect",
    "moveTo",
    "lineTo",
    "clip",
    "fill",
    "stroke",
    "fillRect",
    "strokeRect",
    "fillText",
    "setLineDash",
    "drawImage",
    "translate",
    "scale",
  ]) {
    ctx[name] = noop;
  }
  return ctx as unknown as CanvasRenderingContext2D;
}

const newSpace = (): number =>
  getHeapSpaceStatistics().find((s) => s.space_name === "new_space")?.space_used_size ?? 0;

describe("allocation on the draw path", () => {
  it("does not grow with the number of clips drawn", { retry: 2 }, () => {
    const view = new TimelineView(fakeCanvas(), {
      theme: testTheme(),
      context: noopContext(),
      requestFrame: () => 1,
      cancelFrame: () => {},
    });
    view.resize(1600, 900, 1);
    view.setModel(withMarkers(generateTimeline()));

    /** Median young-generation growth (bytes) of a frame, after warming up the same frames. */
    const perFrame = (pose: () => void, step: () => void): { bytes: number; clips: number } => {
      for (let pass = 0; pass < 2; pass++) {
        pose();
        const grown: number[] = [];
        for (let i = 0; i < 300; i++) {
          step();
          view.invalidate();
          const before = newSpace();
          view.flush();
          const after = newSpace();
          if (after >= before) grown.push(after - before);
        }
        if (pass === 1) {
          expect(grown.length).toBeGreaterThan(150);
          grown.sort((a, b) => a - b);
          return { bytes: grown[grown.length >> 1] ?? 0, clips: view.stats.visibleItems };
        }
      }
      return { bytes: 0, clips: 0 };
    };

    const few = perFrame(
      () => view.setViewState({ pxPerSec: 20, scrollX: 600 * F }),
      () => {},
    );
    const many = perFrame(
      () => view.setViewState({ pxPerSec: 0.5, scrollX: 0 }),
      () => {},
    );
    const pan = perFrame(
      () => view.setViewState({ pxPerSec: 20, scrollX: 600 * F }),
      () => view.scrollBy(3),
    );
    console.info(
      `draw path, median per frame: ${few.bytes} B with ${few.clips} clips, ${many.bytes} B with ` +
        `${many.clips} clips, ${pan.bytes} B panning`,
    );
    expect(many.clips).toBeGreaterThan(20 * few.clips);
    // Nothing per clip: thousands more clips on screen cost (almost) no more.
    expect(many.bytes - few.bytes).toBeLessThan(1024);
    // A small constant per frame, panning included (labels and ticks are cached).
    expect(few.bytes).toBeLessThan(8 * 1024);
    expect(pan.bytes).toBeLessThan(8 * 1024);
  });
});
