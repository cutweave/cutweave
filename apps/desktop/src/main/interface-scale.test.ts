// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import { INTERFACE_SCALES } from "../../../../packages/preferences/src/index.ts";
import { density, size } from "../../../../packages/tokens/src/generated/tokens.ts";
import {
  applyInterfaceScale,
  cssToScreen,
  titleBarHeightFor,
  toolbarHeight,
  type Zoomable,
} from "./interface-scale.ts";
import { TITLE_BAR_HEIGHT, titleBarOptions, titleBarOverlay } from "./title-bar.ts";

function contents(factor = 1, destroyed = false) {
  const self = {
    factor,
    isDestroyed: () => destroyed,
    getZoomFactor: () => self.factor,
    setZoomFactor: vi.fn((next: number) => {
      self.factor = next;
    }),
  };
  return self satisfies Zoomable;
}

describe("interface scale (WP-2H.09)", () => {
  it("sets the window's zoom factor to the scale", () => {
    for (const scale of INTERFACE_SCALES) {
      const c = contents(scale === 100 ? 1.5 : 1);
      applyInterfaceScale(c, scale);
      expect(c.factor).toBeCloseTo(scale / 100, 6);
    }
  });

  it("leaves a window that already has the factor, and a destroyed one, alone", () => {
    const same = contents(1.25);
    applyInterfaceScale(same, 125);
    expect(same.setZoomFactor).not.toHaveBeenCalled();
    const gone = contents(1, true);
    applyInterfaceScale(gone, 150);
    expect(gone.setZoomFactor).not.toHaveBeenCalled();
  });

  it("keeps the OS window controls as tall as the top bar at every scale and density", () => {
    expect(toolbarHeight("standard")).toBe(Number.parseFloat(size.toolbar));
    expect(toolbarHeight("comfortable")).toBe(Number.parseFloat(density.comfortable.toolbar));
    expect(titleBarHeightFor(100, "standard")).toBe(TITLE_BAR_HEIGHT);
    expect(titleBarHeightFor(125, "standard")).toBe(50);
    expect(titleBarHeightFor(90, "standard")).toBe(36);
    expect(titleBarHeightFor(100, "comfortable")).toBe(44);
    expect(titleBarHeightFor(125, "comfortable")).toBe(55);
    expect(titleBarHeightFor(150, "comfortable")).toBe(66);
  });

  it("gives the overlay and the macOS traffic lights that height", () => {
    expect(titleBarOverlay("dark").height).toBe(TITLE_BAR_HEIGHT);
    expect(titleBarOverlay("light", 55)).toEqual({ ...titleBarOverlay("light"), height: 55 });
    expect(titleBarOptions("win32", "dark", 50).titleBarOverlay).toEqual(
      titleBarOverlay("dark", 50),
    );
    // 14 px traffic lights, centered in a 50 px bar.
    expect(titleBarOptions("darwin", "dark", 50).trafficLightPosition).toEqual({ x: 12, y: 18 });
  });

  it("turns a page's CSS pixels into screen points", () => {
    expect(cssToScreen(400, 125)).toBe(500);
    expect(cssToScreen(400, 100)).toBe(400);
    expect(cssToScreen(400, 90)).toBe(360);
  });
});
