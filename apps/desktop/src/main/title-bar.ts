// SPDX-License-Identifier: GPL-3.0-or-later
// The custom title bar (WP-1M.09, arch §2 "Desktop shell"). The editor's top bar is the title
// bar; the OS keeps drawing the window controls so Snap Layouts, Aero snap and the accessibility
// tree of the caption buttons keep working:
//
// - Windows and Linux: `titleBarStyle: "hidden"` with a window-controls overlay (minimize,
//   maximize/restore, close) drawn by the OS in the app's frame color. Its colors come from the
//   design tokens of the theme the editor shows, its height from the top bar's `toolbar` token.
//   There is no native menu bar; the editor draws File, Edit, View and Help.
// - macOS: `hiddenInset` keeps the traffic lights over the top bar and the native app menu stays
//   at the top of the screen. `titleBarOverlay: true` turns on the Window Controls Overlay API
//   there too, so the page reserves the traffic lights' area the same way on every OS.
//
// Pure, unit-tested. Token values are read from the generated TS constants (the same import the
// test-support fake engine uses for the protocol's generated schema hash).

import type { BrowserWindowConstructorOptions, TitleBarOverlayOptions } from "electron";
import { colors, size, spacing } from "../../../../packages/tokens/src/generated/tokens.ts";
import type { ThemeId } from "../shared/api.ts";

const px = (value: string): number => Number.parseFloat(value);

/** Height of the top bar (the `toolbar` token): the overlay matches it exactly. */
export const TITLE_BAR_HEIGHT = px(size.toolbar);

/** Diameter of the macOS traffic-light buttons (an OS constant), for centering them. */
const TRAFFIC_LIGHT_SIZE = 14;

/** Whether the OS draws an overlay whose colors the app sets (not on macOS). */
export function hasThemedOverlay(platform: NodeJS.Platform): boolean {
  return platform !== "darwin";
}

/** The overlay for `theme`: the app frame color, the primary text color for the glyphs. */
export function titleBarOverlay(
  theme: ThemeId,
): Required<Pick<TitleBarOverlayOptions, "color" | "symbolColor" | "height">> {
  return {
    color: colors[theme]["surface-0"],
    symbolColor: colors[theme]["text-primary"],
    height: TITLE_BAR_HEIGHT,
  };
}

/** Window options for the title bar on `platform`; the overlay starts in `theme`. */
export function titleBarOptions(
  platform: NodeJS.Platform,
  theme: ThemeId = "dark",
): Pick<
  BrowserWindowConstructorOptions,
  "titleBarStyle" | "titleBarOverlay" | "trafficLightPosition" | "backgroundColor"
> {
  // The frame color shows before the first paint, so the window never flashes white.
  const backgroundColor = colors[theme]["surface-0"];
  if (platform === "darwin") {
    return {
      titleBarStyle: "hiddenInset",
      titleBarOverlay: true,
      trafficLightPosition: {
        x: px(spacing["space-3"]),
        y: Math.round((TITLE_BAR_HEIGHT - TRAFFIC_LIGHT_SIZE) / 2),
      },
      backgroundColor,
    };
  }
  return { titleBarStyle: "hidden", titleBarOverlay: titleBarOverlay(theme), backgroundColor };
}
