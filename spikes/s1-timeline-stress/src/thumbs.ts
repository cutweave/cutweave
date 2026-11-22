// SPDX-License-Identifier: GPL-3.0-or-later
// Generates synthetic thumbnail bitmaps through the same path real ones would take:
// OffscreenCanvas -> PNG blob -> createImageBitmap (decode), i.e. a cached ImageBitmap tile.
import { mulberry32 } from "./model.ts";
import type { Theme } from "./theme.ts";

export const THUMB_W = 96;
export const THUMB_H = 54;

export async function makeThumbnails(count: number, theme: Theme): Promise<ImageBitmap[]> {
  const out: ImageBitmap[] = [];
  const { edge, body } = theme.clip.media;
  for (let i = 0; i < count; i++) {
    const rnd = mulberry32(1000 + i);
    const oc = new OffscreenCanvas(THUMB_W, THUMB_H);
    const g = oc.getContext("2d");
    if (!g) throw new Error("OffscreenCanvas 2d unavailable");
    g.fillStyle = body;
    g.fillRect(0, 0, THUMB_W, THUMB_H);
    g.globalAlpha = 0.35;
    g.fillStyle = edge;
    for (let s = 0; s < 4; s++) {
      g.beginPath();
      g.arc(rnd() * THUMB_W, rnd() * THUMB_H, 6 + rnd() * 22, 0, Math.PI * 2);
      g.fill();
    }
    g.globalAlpha = 1;
    g.fillStyle = theme.textMuted;
    g.font = `10px ${theme.fontMono}`;
    g.fillText(String(i), 4, THUMB_H - 5);
    out.push(await createImageBitmap(await oc.convertToBlob({ type: "image/png" })));
  }
  return out;
}
