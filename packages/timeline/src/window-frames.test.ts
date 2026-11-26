// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2H.15: a timeline in a detached panel's window takes its frames from that window (a hidden
// main window's frames stop), and follows its canvas when it moves between windows.
import { afterEach, describe, expect, it, vi } from "vitest";
import { generateSmallTimeline } from "./fixtures.ts";
import { spyContext, testTheme } from "./test-support.ts";
import { TimelineView } from "./timeline-view.ts";

afterEach(() => {
  vi.restoreAllMocks();
  for (const frame of document.querySelectorAll("iframe")) frame.remove();
});

/** A second window, as a detached panel's. */
function secondWindow(): Window {
  const frame = document.createElement("iframe");
  document.body.append(frame);
  return frame.contentWindow as Window;
}

/** A canvas in `doc`, inside a box it fills (what `bind()` observes). */
function canvasIn(doc: Document): HTMLCanvasElement {
  const box = doc.createElement("div");
  const canvas = doc.createElement("canvas");
  box.append(canvas);
  doc.body.append(box);
  return canvas;
}

describe("a timeline's frames", () => {
  it("come from its canvas's window", () => {
    const other = secondWindow();
    const otherFrames = vi.spyOn(other, "requestAnimationFrame");
    const mainFrames = vi.spyOn(window, "requestAnimationFrame");
    const view = new TimelineView(canvasIn(other.document), {
      theme: testTheme(),
      context: spyContext().ctx,
    });
    view.setModel(generateSmallTimeline());
    view.resize(400, 200, 1);
    view.invalidate();
    expect(otherFrames).toHaveBeenCalledTimes(1);
    expect(mainFrames).not.toHaveBeenCalled();
    view.destroy();
  });

  it("move with the canvas to another window, and a frame asked of the old one is asked again", () => {
    const other = secondWindow();
    const canvas = canvasIn(document);
    const mainCancel = vi.spyOn(window, "cancelAnimationFrame");
    const mainFrames = vi.spyOn(window, "requestAnimationFrame");
    const view = new TimelineView(canvas, { theme: testTheme(), context: spyContext().ctx });
    view.bind();
    view.setModel(generateSmallTimeline());
    view.resize(400, 200, 1);
    view.invalidate();
    // One frame pending, asked of the main window (where the canvas is).
    expect(mainFrames).toHaveBeenCalledTimes(1);

    // The dock moves the panel's DOM into the other window (a detached panel).
    const otherFrames = vi.spyOn(other, "requestAnimationFrame");
    other.document.body.append(canvas.parentElement as HTMLElement);
    view.invalidate();
    // The main window's pending frame may never come there: it is dropped and asked of the new one.
    expect(mainCancel).toHaveBeenCalledTimes(1);
    expect(otherFrames).toHaveBeenCalledTimes(1);
    // `rehome` re-binds its listeners in the new window; the pointer listeners came along.
    const addOther = vi.spyOn(other, "addEventListener");
    view.rehome();
    view.rehome(); // nothing more when it is already there
    expect(mainFrames).toHaveBeenCalledTimes(1);
    expect(addOther.mock.calls.length).toBeLessThanOrEqual(1);
    view.destroy();
  });
});
