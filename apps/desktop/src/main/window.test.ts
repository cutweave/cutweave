// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { mainWindowPreferences } from "./window-prefs.ts";
import {
  DEFAULT_WINDOW_STATE,
  fitToDisplays,
  MIN_WINDOW,
  parseWindowState,
} from "./window-state.ts";

describe("mainWindowPreferences", () => {
  it("keeps the ADR-001 posture: isolated, no Node, non-sandboxed preload only", () => {
    expect(mainWindowPreferences("/x/preload.cjs")).toMatchObject({
      preload: "/x/preload.cjs",
      contextIsolation: true,
      nodeIntegration: false,
      nodeIntegrationInSubFrames: false,
      sandbox: false,
      webSecurity: true,
      allowRunningInsecureContent: false,
      webviewTag: false,
    });
  });
});

describe("window state", () => {
  it("parses valid state and rejects junk", () => {
    const raw = JSON.stringify({ bounds: { x: 1, y: 2, width: 3, height: 4 }, maximized: true });
    expect(parseWindowState(raw)).toEqual({
      bounds: { x: 1, y: 2, width: 3, height: 4 },
      maximized: true,
    });
    expect(parseWindowState("nope")).toBeNull();
    expect(parseWindowState(JSON.stringify({ bounds: { x: "1" } }))).toBeNull();
  });

  const display = { x: 0, y: 0, width: 1920, height: 1080 };

  it("keeps on-screen positions and enforces the minimum size", () => {
    const fitted = fitToDisplays(
      { bounds: { x: 100, y: 100, width: 100, height: 100 }, maximized: false },
      [display],
    );
    expect(fitted.bounds).toEqual({
      x: 100,
      y: 100,
      width: MIN_WINDOW.width,
      height: MIN_WINDOW.height,
    });
  });

  it("drops the position when the display is gone", () => {
    const fitted = fitToDisplays(
      { bounds: { x: 5000, y: 0, width: 1200, height: 800 }, maximized: true },
      [display],
    );
    expect(fitted.bounds).toEqual({ width: 1200, height: 800 });
    expect(fitted.maximized).toBe(true);
    expect(fitToDisplays(DEFAULT_WINDOW_STATE, [display]).bounds.width).toBe(1440);
  });
});
