// SPDX-License-Identifier: GPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { PNG } from "pngjs";
import { compareImages } from "./compare.ts";

// Unit tests for the SSIM/PSNR comparator (no browser involved).
function image(w: number, h: number, paint: (x: number, y: number) => [number, number, number]) {
  const png = new PNG({ width: w, height: h });
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const [r, g, b] = paint(x, y);
      const i = (y * w + x) * 4;
      png.data[i] = r;
      png.data[i + 1] = g;
      png.data[i + 2] = b;
      png.data[i + 3] = 255;
    }
  }
  return PNG.sync.write(png);
}
const gradient = (x: number, y: number): [number, number, number] => [
  (x * 3) % 256,
  (y * 5) % 256,
  (x + y) % 256,
];

test("identical images pass with SSIM 1 and infinite PSNR", () => {
  const a = image(96, 64, gradient);
  const r = compareImages(a, a);
  expect(r.pass).toBe(true);
  expect(r.ssim).toBeCloseTo(1, 6);
  expect(r.psnr).toBe(Number.POSITIVE_INFINITY);
});

test("a one-level noise-level shift stays within tolerance", () => {
  const a = image(96, 64, gradient);
  const b = image(96, 64, (x, y) => {
    const [r, g, bl] = gradient(x, y);
    return [Math.min(255, r + 1), g, bl];
  });
  const r = compareImages(a, b);
  expect(r.psnr).toBeGreaterThan(40);
  expect(r.pass).toBe(true);
});

test("a changed region fails", () => {
  const a = image(96, 64, gradient);
  const b = image(96, 64, (x, y) =>
    x > 20 && x < 70 && y > 10 && y < 50 ? [0, 0, 0] : gradient(x, y),
  );
  const r = compareImages(a, b);
  expect(r.pass).toBe(false);
  expect(r.diff).toBeDefined();
});

test("a size change fails", () => {
  const r = compareImages(image(96, 64, gradient), image(96, 65, gradient));
  expect(r).toMatchObject({ pass: false, sizeMatch: false });
});
