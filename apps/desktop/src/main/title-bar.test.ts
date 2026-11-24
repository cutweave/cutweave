// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { colors, size } from "../../../../packages/tokens/src/generated/tokens.ts";
import {
  hasThemedOverlay,
  TITLE_BAR_HEIGHT,
  titleBarOptions,
  titleBarOverlay,
} from "./title-bar.ts";

describe("title bar (WP-1M.09)", () => {
  it("is as tall as the top bar's toolbar token", () => {
    expect(size.toolbar).toBe(`${TITLE_BAR_HEIGHT}px`);
    expect(titleBarOverlay("dark").height).toBe(TITLE_BAR_HEIGHT);
  });

  it("colors the OS overlay from the theme's frame and text tokens", () => {
    expect(titleBarOverlay("dark")).toEqual({
      color: colors.dark["surface-0"],
      symbolColor: colors.dark["text-primary"],
      height: TITLE_BAR_HEIGHT,
    });
    expect(titleBarOverlay("light")).toEqual({
      color: colors.light["surface-0"],
      symbolColor: colors.light["text-primary"],
      height: TITLE_BAR_HEIGHT,
    });
    expect(titleBarOverlay("light").color).not.toBe(titleBarOverlay("dark").color);
  });

  it("hides the native title bar and keeps OS-drawn controls on Windows and Linux", () => {
    for (const platform of ["win32", "linux"] as const) {
      expect(titleBarOptions(platform)).toEqual({
        titleBarStyle: "hidden",
        titleBarOverlay: titleBarOverlay("dark"),
        backgroundColor: colors.dark["surface-0"],
      });
      expect(hasThemedOverlay(platform)).toBe(true);
    }
    expect(titleBarOptions("win32", "light").titleBarOverlay).toEqual(titleBarOverlay("light"));
  });

  it("keeps the traffic lights on macOS, centered in the top bar, with the overlay API on", () => {
    const mac = titleBarOptions("darwin");
    expect(mac.titleBarStyle).toBe("hiddenInset");
    expect(mac.titleBarOverlay).toBe(true);
    expect(mac.trafficLightPosition).toEqual({ x: 12, y: 13 });
    expect(hasThemedOverlay("darwin")).toBe(false);
  });
});
