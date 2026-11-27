// SPDX-License-Identifier: GPL-3.0-or-later
// Settings > Appearance > Interface scale and Density, as main applies them (WP-2H.09, flow F26).
//
// The scale is the zoom factor of every window of the app: the editor window and its detached
// panels' windows (ADR-0031). Chromium lays the page out in fewer or more CSS pixels and raises
// or lowers `devicePixelRatio` by the same factor, so canvases that size their backing store as
// CSS size × devicePixelRatio (the preview, the timeline) stay sharp, and pointer positions,
// which the page gets in CSS pixels, stay exact.
//
// The OS draws the window controls over the top bar at a height main gives it in screen points,
// which the zoom does not change: main keeps that height equal to the top bar's (the `toolbar`
// token of the density × the zoom factor).
//
// Pure, unit-tested; index.ts does the wiring.
import {
  type DensityChoice,
  type InterfaceScale,
  zoomFactorFor,
} from "../../../../packages/preferences/src/index.ts";
import { density, size } from "../../../../packages/tokens/src/generated/tokens.ts";

const px = (value: string): number => Number.parseFloat(value);

/** The part of a `webContents` the zoom is set through. */
export interface Zoomable {
  isDestroyed(): boolean;
  getZoomFactor(): number;
  setZoomFactor(factor: number): void;
}

/**
 * Sets the zoom factor of `contents` for `scale`. A destroyed window is skipped, and one that
 * already has the factor is left alone, so a repeated call does not lay the page out again.
 */
export function applyInterfaceScale(contents: Zoomable, scale: InterfaceScale): void {
  if (contents.isDestroyed()) return;
  const factor = zoomFactorFor(scale);
  if (Math.abs(contents.getZoomFactor() - factor) < 1e-6) return;
  contents.setZoomFactor(factor);
}

/** The top bar's height in CSS pixels in `choice`: the `toolbar` token of that density. */
export function toolbarHeight(choice: DensityChoice): number {
  return px(choice === "comfortable" ? density.comfortable.toolbar : size.toolbar);
}

/**
 * The height of the OS window-controls overlay, in screen points: as tall as the top bar is on
 * screen at this scale and density.
 */
export function titleBarHeightFor(scale: InterfaceScale, choice: DensityChoice): number {
  return Math.round(toolbarHeight(choice) * zoomFactorFor(scale));
}

/**
 * A size the page gave in its own CSS pixels (a detached panel's saved `innerWidth` and
 * `innerHeight`), in screen points: at 125% a 400 px wide page is 500 points wide.
 */
export function cssToScreen(value: number, scale: InterfaceScale): number {
  return Math.round(value * zoomFactorFor(scale));
}
