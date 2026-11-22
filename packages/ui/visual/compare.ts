// SPDX-License-Identifier: GPL-3.0-or-later

import { PNG } from "pngjs";
import { ssim } from "ssim.js";
import { PSNR_MIN, SSIM_MIN } from "./shared.ts";

export interface Comparison {
  pass: boolean;
  sizeMatch: boolean;
  ssim: number;
  /** dB over RGB; Infinity for identical images. */
  psnr: number;
  /** PNG of the amplified absolute difference (same size as the inputs), when sizes match. */
  diff?: Buffer;
}

/** Mean-SSIM (ssim.js, Bezkrovny integral-image variant) and PSNR over RGB. */
export function compareImages(
  expectedPng: Buffer,
  actualPng: Buffer,
  limits = { ssim: SSIM_MIN, psnr: PSNR_MIN },
): Comparison {
  const a = PNG.sync.read(expectedPng);
  const b = PNG.sync.read(actualPng);
  if (a.width !== b.width || a.height !== b.height) {
    return { pass: false, sizeMatch: false, ssim: 0, psnr: 0 };
  }
  const { width, height } = a;
  let sq = 0;
  const diff = new PNG({ width, height });
  for (let i = 0; i < a.data.length; i += 4) {
    let any = 0;
    for (let c = 0; c < 3; c++) {
      const d = (a.data[i + c] ?? 0) - (b.data[i + c] ?? 0);
      sq += d * d;
      any = Math.max(any, Math.abs(d));
      diff.data[i + c] = Math.min(255, Math.abs(d) * 8);
    }
    diff.data[i + 3] = any === 0 ? 0 : 255;
  }
  const mse = sq / (width * height * 3);
  const psnr = mse === 0 ? Number.POSITIVE_INFINITY : 10 * Math.log10((255 * 255) / mse);
  const { mssim } = ssim(
    { data: new Uint8ClampedArray(a.data), width, height },
    { data: new Uint8ClampedArray(b.data), width, height },
  );
  return {
    pass: mssim >= limits.ssim && psnr >= limits.psnr,
    sizeMatch: true,
    ssim: mssim,
    psnr,
    diff: PNG.sync.write(diff),
  };
}
