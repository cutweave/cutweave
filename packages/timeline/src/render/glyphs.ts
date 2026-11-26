// SPDX-License-Identifier: GPL-3.0-or-later
// 24x24 icon geometry (the same glyphs as the design-system components), drawn as stroked Path2D
// objects that are built once per path and reused.
import type { HeaderToggle } from "../geometry.ts";
import type { ItemKind } from "../model.ts";

const ICON_RECT = (x: number, y: number, w: number, h: number, r: number): string =>
  `M${x + r} ${y}h${w - 2 * r}a${r} ${r} 0 0 1 ${r} ${r}v${h - 2 * r}a${r} ${r} 0 0 1-${r} ${r}h-${w - 2 * r}a${r} ${r} 0 0 1-${r}-${r}v-${h - 2 * r}a${r} ${r} 0 0 1 ${r}-${r}z`;

export const KIND_ICON: Readonly<Record<ItemKind, string>> = {
  media: `${ICON_RECT(3, 3, 18, 18, 2)}M7 3v18M17 3v18`,
  audio: "M11 5 6 9H2v6h4l5 4zM15.5 8.5a5 5 0 0 1 0 7",
  graphics: "M4 7V5h16v2M9 19h6M12 5v14",
  captions: `${ICON_RECT(3, 5, 18, 14, 2)}M7 15h4M15 15h2`,
  effects: "M15 4V2M15 16v-2M8 9h2M20 9h2M3 21l9-9",
};

export const TOGGLE_ICON: Readonly<Record<HeaderToggle, string>> = {
  lock: `${ICON_RECT(3, 11, 18, 11, 2)}M7 11V7a5 5 0 0 1 10 0v4`,
  mute: "M11 5 6 9H2v6h4l5 4zM22 9l-6 6M16 9l6 6",
  hide: "M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7-10-7-10-7zM15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
};

/** Path2D objects per SVG path string; empty where Path2D does not exist (Node tests). */
export class GlyphCache {
  private readonly paths = new Map<string, Path2D>();

  get(svg: string): Path2D | null {
    if (typeof Path2D === "undefined") return null;
    let p = this.paths.get(svg);
    if (p === undefined) {
      p = new Path2D(svg);
      this.paths.set(svg, p);
    }
    return p;
  }

  clear(): void {
    this.paths.clear();
  }
}

/** Strokes `svg` (a 24-unit icon) centered at `(cx, cy)`, `size` CSS px, in `color`. */
export function drawGlyph(
  ctx: CanvasRenderingContext2D,
  glyphs: GlyphCache,
  svg: string,
  cx: number,
  cy: number,
  size: number,
  color: string,
): boolean {
  const p = glyphs.get(svg);
  if (!p) return false;
  const s = size / 24;
  ctx.save();
  ctx.translate(cx - size / 2, cy - size / 2);
  ctx.scale(s, s);
  ctx.strokeStyle = color;
  ctx.lineWidth = 2;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.stroke(p);
  ctx.restore();
  return true;
}
