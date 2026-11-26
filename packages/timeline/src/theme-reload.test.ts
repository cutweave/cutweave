// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2H.15: a timeline moved into a detached window whose stylesheets are still loading (the
// window copies them as links) keeps drawing with the tokens it has, instead of throwing (which
// took the whole editor down), and reads them again once a stylesheet there has loaded.
import { afterEach, describe, expect, it, vi } from "vitest";
import { generateSmallTimeline } from "./fixtures.ts";
import { spyContext, testTheme } from "./test-support.ts";

const readTheme = vi.hoisted(() => vi.fn());
vi.mock("./theme.ts", async (original) => ({
  ...(await original<typeof import("./theme.ts")>()),
  readTheme,
}));

const { TimelineView } = await import("./timeline-view.ts");

afterEach(() => {
  readTheme.mockReset();
  for (const frame of document.querySelectorAll("iframe")) frame.remove();
});

describe("a timeline's tokens in another window", () => {
  it("are kept while that window's stylesheets load, and read again once one has", () => {
    readTheme.mockImplementation(() => testTheme());
    const box = document.createElement("div");
    const canvas = document.createElement("canvas");
    box.append(canvas);
    document.body.append(box);
    const view = new TimelineView(canvas, { context: spyContext().ctx });
    view.bind();
    view.setModel(generateSmallTimeline());

    const frame = document.createElement("iframe");
    document.body.append(frame);
    const other = (frame.contentWindow as Window).document;
    other.body.append(box);
    readTheme.mockImplementation(() => {
      throw new Error("@cutweave/timeline: missing design token --surface-0");
    });
    expect(() => view.rehome()).not.toThrow();

    // A stylesheet of the new window loads: the tokens are read from it.
    readTheme.mockClear();
    readTheme.mockImplementation(() => testTheme());
    const link = other.createElement("link");
    other.head.append(link);
    link.dispatchEvent(new Event("load"));
    expect(readTheme).toHaveBeenCalledTimes(1);
    expect(readTheme).toHaveBeenCalledWith(other.documentElement);
    // Only once: the wait ends with the read.
    link.dispatchEvent(new Event("load"));
    expect(readTheme).toHaveBeenCalledTimes(1);
    view.destroy();
  });
});
