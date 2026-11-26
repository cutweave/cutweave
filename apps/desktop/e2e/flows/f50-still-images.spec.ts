// SPDX-License-Identifier: GPL-3.0-or-later
// F50 Still images (docs/user-flows.md; WP-2F.05a): a photo taken with the camera turned (a JPEG
// whose EXIF orientation says "turn me a quarter clockwise") is imported from the bin's Import
// button, dropped on the timeline and shows upright in the preview, in its own colours: checked
// from the pixels on screen. The clip is 5 s long (a still has no length of its own).
//
// The photo is written by the flow (a baseline JPEG of six flat 8 × 8 blocks with an EXIF
// block): no media is committed. Needs an engine built with FFmpeg (`just e2e-flows` builds it
// when the bundle is set up); without it the flow skips and says why.
import { writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  pickFiles,
  snapshot,
  workspace,
} from "./helpers.ts";

type Rgb = readonly [number, number, number];

const RED: Rgb = [200, 30, 30];
const GREEN: Rgb = [30, 200, 30];
const BLUE: Rgb = [30, 30, 200];
const WHITE: Rgb = [240, 240, 240];
const DARK: Rgb = [20, 20, 20];
const YELLOW: Rgb = [220, 220, 40];

/** JFIF's full-range BT.601 Y'CbCr of an R'G'B' colour. */
function ycbcr([r, g, b]: Rgb): Rgb {
  const clamp = (v: number) => Math.min(255, Math.max(0, Math.round(v)));
  return [
    clamp(0.299 * r + 0.587 * g + 0.114 * b),
    clamp(128 - 0.168736 * r - 0.331264 * g + 0.5 * b),
    clamp(128 + 0.5 * r - 0.418688 * g - 0.081312 * b),
  ];
}

/**
 * A baseline JFIF file of `blocksW × blocksH` flat 8 × 8 blocks, Y'CbCr 4:4:4, with an EXIF
 * block holding one tag: `orientation`. Only DC coefficients, unit quantisation: every block
 * decodes to the colour it was given. The same writer as the engine's tests
 * (`cutweave_media::testing::stills::jpeg`).
 */
function jpeg(
  blocksW: number,
  blocksH: number,
  blocks: readonly Rgb[],
  orientation: number,
): Buffer {
  const out: number[] = [0xff, 0xd8];
  const segment = (marker: number, data: readonly number[]) => {
    out.push(0xff, marker, (data.length + 2) >> 8, (data.length + 2) & 0xff, ...data);
  };
  segment(0xe0, [0x4a, 0x46, 0x49, 0x46, 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]);
  // "Exif\0\0", then a little-endian TIFF structure: one directory, one entry (tag 0x0112,
  // SHORT, count 1).
  segment(0xe1, [
    ...[0x45, 0x78, 0x69, 0x66, 0, 0],
    ...[0x49, 0x49, 42, 0, 8, 0, 0, 0],
    ...[1, 0],
    ...[0x12, 0x01, 3, 0, 1, 0, 0, 0, orientation, 0, 0, 0],
    ...[0, 0, 0, 0],
  ]);
  segment(0xdb, [0, ...new Array<number>(64).fill(1)]);
  const [h, w] = [blocksH * 8, blocksW * 8];
  segment(0xc0, [8, h >> 8, h & 0xff, w >> 8, w & 0xff, 3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0]);
  // DC table 0: the standard luminance table. AC table 0: two 2-bit codes, `00` ends a block.
  const dcLengths = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
  const dcCategories = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
  segment(0xc4, [0x00, ...dcLengths, ...dcCategories]);
  const acLengths = [0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
  segment(0xc4, [0x10, ...acLengths, 0x00, 0x01]);
  segment(0xda, [3, 1, 0x00, 2, 0x00, 3, 0x00, 0, 63, 0]);

  let acc = 0;
  let bits = 0;
  const put = (code: number, length: number) => {
    for (let i = length - 1; i >= 0; i -= 1) {
      acc = (acc << 1) | ((code >> i) & 1);
      bits += 1;
      if (bits === 8) {
        out.push(acc);
        if (acc === 0xff) out.push(0);
        acc = 0;
        bits = 0;
      }
    }
  };
  const pred = [0, 0, 0];
  for (const block of blocks.slice(0, blocksW * blocksH)) {
    for (let c = 0; c < 3; c += 1) {
      // The DC coefficient of a flat block (level-shifted by 128) is 8 times its sample.
      const dc = ((block[c] ?? 128) - 128) * 8;
      const diff = dc - (pred[c] ?? 0);
      pred[c] = dc;
      const category = diff === 0 ? 0 : 32 - Math.clz32(Math.abs(diff));
      if (category === 0) put(0b00, 2);
      else if (category <= 5) put(0b010 + category - 1, 3);
      else put((1 << (category - 2)) - 2, category - 2);
      if (category > 0) put(diff >= 0 ? diff : diff + (1 << category) - 1, category);
      put(0b00, 2);
    }
  }
  while (bits !== 0) put(1, 1);
  out.push(0xff, 0xd9);
  return Buffer.from(out);
}

interface Seen {
  /** The picture's box in the preview (where enough pixels of a row or column are lit). */
  readonly width: number;
  readonly height: number;
  /** The colour in the middle of each cell of a 2 × 3 grid over that box, row by row. */
  readonly cells: Rgb[];
}

/**
 * What the person sees in the preview: a screenshot of its canvas, decoded in the page. The
 * picture is found as the box of the lit rows and columns (the sequence's background is black),
 * then read as two columns by three rows.
 */
async function seen(page: Page): Promise<Seen | null> {
  const box = await page.locator("canvas.cw-preview__canvas").boundingBox();
  if (box === null) return null;
  const png = await page.screenshot({
    clip: {
      x: Math.round(box.x),
      y: Math.round(box.y),
      width: Math.round(box.width),
      height: Math.round(box.height),
    },
    animations: "allow",
    caret: "initial",
  });
  return page.evaluate(async (base64) => {
    const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const ctx = canvas.getContext("2d");
    if (ctx === null) throw new Error("no 2D context");
    ctx.drawImage(bitmap, 0, 0);
    const { width, height } = bitmap;
    const { data } = ctx.getImageData(0, 0, width, height);
    const lit = (x: number, y: number) => {
      const i = (y * width + x) * 4;
      return Math.max(data[i] ?? 0, data[i + 1] ?? 0, data[i + 2] ?? 0) > 96;
    };
    // A row or column belongs to the picture when a tenth of it is lit (the overlay's thin
    // lines and its timecode are not).
    const cols: number[] = [];
    for (let x = 0; x < width; x += 1) {
      let n = 0;
      for (let y = 0; y < height; y += 1) if (lit(x, y)) n += 1;
      if (n > height / 10) cols.push(x);
    }
    const rows: number[] = [];
    for (let y = 0; y < height; y += 1) {
      let n = 0;
      for (let x = 0; x < width; x += 1) if (lit(x, y)) n += 1;
      if (n > width / 20) rows.push(y);
    }
    const [x0, x1] = [cols[0], cols[cols.length - 1]];
    const [y0, y1] = [rows[0], rows[rows.length - 1]];
    if (x0 === undefined || x1 === undefined || y0 === undefined || y1 === undefined) return null;
    const [w, h] = [x1 - x0 + 1, y1 - y0 + 1];
    const cells: [number, number, number][] = [];
    for (let row = 0; row < 3; row += 1) {
      for (let col = 0; col < 2; col += 1) {
        const i =
          (Math.round(y0 + ((row + 0.5) * h) / 3) * width +
            Math.round(x0 + ((col + 0.5) * w) / 2)) *
          4;
        cells.push([data[i] ?? 0, data[i + 1] ?? 0, data[i + 2] ?? 0]);
      }
    }
    return { width: w, height: h, cells };
  }, png.toString("base64"));
}

function close(a: Rgb, b: Rgb): boolean {
  return a.every((v, i) => Math.abs(v - (b[i] ?? 0)) <= 24);
}

test.describe("F50 still images", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a photo taken sideways shows upright, in its colours, for 5 seconds", async () => {
    test.setTimeout(240_000);
    const info = test.info();
    const work = workspace();
    const {
      app,
      page,
      close: closeApp,
    } = await launchFlowApp(work, undefined, { realMedia: true });
    try {
      const caps = await editorCall<{ available?: boolean }>(page, "media.capabilities");
      test.skip(
        caps.available !== true,
        "this engine has no media backend; build it with --features cutweave-engine/ffmpeg",
      );
      await createProject(app, page, "Stills", work.dir);

      // As stored the photo is 24 × 16 (landscape): red, green, blue over white, dark, yellow.
      // Its EXIF orientation is 6: the camera was turned, show it a quarter turn clockwise.
      const name = "turned.jpg";
      const file = path.join(work.media, name);
      writeFileSync(file, jpeg(3, 2, [RED, GREEN, BLUE, WHITE, DARK, YELLOW].map(ycbcr), 6));

      await pickFiles(app, [file]);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(1);
      await expect(mediaCard(page, name)).toBeVisible();
      const listed = await editorCall<{ media: { kind: string }[] }>(page, "media.list");
      expect(listed.media.map((m) => m.kind)).toEqual(["image"]);

      // On the timeline a still is 5 s long.
      await dropCard(page, name, "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
      await editorCaughtUp(page);
      const clip = (await layout(page))[0]?.clips[0];
      expect((clip?.[1] ?? 0) - (clip?.[0] ?? 0)).toBeCloseTo(5, 1);

      // The preview shows it upright: taller than wide (16 × 24), the stored left column on
      // top: white and red, dark and green, yellow and blue.
      const surface = page.locator(".cw-preview__surface");
      await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 20_000 });
      const expected = [WHITE, RED, DARK, GREEN, YELLOW, BLUE];
      await expect
        .poll(
          async () => {
            const picture = await seen(page);
            if (picture === null) return "no picture";
            if (picture.height < picture.width * 1.3) {
              return `not upright: ${picture.width} × ${picture.height}`;
            }
            const wrong = expected.flatMap((colour, i) =>
              close(picture.cells[i] ?? [0, 0, 0], colour)
                ? []
                : [`cell ${i}: ${String(picture.cells[i])}, expected ${String(colour)}`],
            );
            return wrong.length === 0 ? "upright" : wrong.join("; ");
          },
          { timeout: 30_000, message: "the photo in the preview" },
        )
        .toBe("upright");
      await snapshot(page, info, "upright photo");
    } finally {
      await closeApp();
      work.remove();
    }
  });
});
