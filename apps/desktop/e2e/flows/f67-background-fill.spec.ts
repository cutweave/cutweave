// SPDX-License-Identifier: GPL-3.0-or-later
// F67 Background fill (docs/user-flows.md; WP-2B.02): a landscape photo sits on a vertical
// (9:16) sequence with black above and below it. In the Inspector's Sequence section, Fill >
// Blurred picture fills the whole frame with a blurred, dimmer copy of the photo; Dim at 100%
// takes it back to black, Undo brings it back; Fill > Color only removes it. Read from the
// pixels on screen and from the document.
//
// The photo is written by the flow (a flat-colour PNG): no media is committed. Needs an engine
// built with FFmpeg (`just e2e-flows` builds it when the bundle is set up); without it the flow
// skips and says why.
import { writeFileSync } from "node:fs";
import path from "node:path";
import { deflateSync } from "node:zlib";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  laneY,
  launchFlowApp,
  layout,
  mediaCards,
  movePlayhead,
  pickFiles,
  sec,
  showTimeline,
  snapshot,
  timelineGeometry,
  workspace,
  xAt,
} from "./helpers.ts";

type Rgb = readonly [number, number, number];

/** The photo's colour: bright, with three different channels. */
const ORANGE: Rgb = [240, 150, 30];
const BOUND = { timeout: 20_000 };

function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let i = 0; i < 8; i += 1) crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/** A `width × height` 8-bit RGB PNG of one colour. */
function png(width: number, height: number, [r, g, b]: Rgb): Buffer {
  const chunk = (kind: string, data: Buffer): Buffer => {
    const body = Buffer.concat([Buffer.from(kind, "latin1"), data]);
    const out = Buffer.alloc(body.length + 8);
    out.writeUInt32BE(data.length, 0);
    body.copy(out, 4);
    out.writeUInt32BE(crc32(body), body.length + 4);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const row = Buffer.alloc(1 + width * 3);
  for (let x = 0; x < width; x += 1) row.set([r, g, b], 1 + x * 3);
  const raw = Buffer.concat(Array.from({ length: height }, () => row));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

interface Seen {
  /** The lit part of the preview: the photo alone, or the whole frame once it is filled. */
  readonly width: number;
  readonly height: number;
  /** The colour in its middle (the photo) and near its top edge (a bar, when filled). */
  readonly middle: Rgb;
  readonly top: Rgb;
}

/**
 * What the person sees in the preview: a screenshot of its canvas, decoded in the page. The lit
 * part is the box of the rows and columns where enough pixels are brighter than the black
 * background.
 */
async function seen(page: Page): Promise<Seen | null> {
  const box = await page.locator("canvas.cw-preview__canvas").boundingBox();
  if (box === null) return null;
  const shot = await page.screenshot({
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
      return Math.max(data[i] ?? 0, data[i + 1] ?? 0, data[i + 2] ?? 0) > 60;
    };
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
      if (n > width / 10) rows.push(y);
    }
    const [x0, x1] = [cols[0], cols[cols.length - 1]];
    const [y0, y1] = [rows[0], rows[rows.length - 1]];
    if (x0 === undefined || x1 === undefined || y0 === undefined || y1 === undefined) return null;
    const [w, h] = [x1 - x0 + 1, y1 - y0 + 1];
    const at = (fx: number, fy: number): [number, number, number] => {
      const i = (Math.round(y0 + fy * h) * width + Math.round(x0 + fx * w)) * 4;
      return [data[i] ?? 0, data[i + 1] ?? 0, data[i + 2] ?? 0];
    };
    // A third of the way across: away from the overlay's centre marks.
    return { width: w, height: h, middle: at(0.3, 0.5), top: at(0.3, 0.06) };
  }, shot.toString("base64"));
}

const near = (a: Rgb, b: Rgb, tolerance = 28): boolean =>
  a.every((v, i) => Math.abs(v - (b[i] ?? 0)) <= tolerance);

/** The first sequence's background as the engine has it. */
async function background(page: Page) {
  const info = await editorCall<{
    sequences: {
      format: {
        width: number;
        height: number;
        background: string;
        background_fill?: { kind: string; radius?: number; dim?: number; source?: string } | null;
      };
    }[];
  }>(page, "project.info");
  const first = info.sequences[0];
  if (first === undefined) throw new Error("the project has no sequence");
  return first.format;
}

/** `"photo"`: only the landscape photo is lit; `"filled"`: the whole vertical frame is. */
async function look(page: Page): Promise<string> {
  const s = await seen(page);
  if (s === null) return "nothing lit";
  if (!near(s.middle, ORANGE)) return `the photo is ${String(s.middle)}`;
  if (s.width > s.height * 1.2) return "photo";
  if (s.height > s.width * 1.5) return "filled";
  return `lit ${s.width} × ${s.height}`;
}

test.describe("F67 background fill", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a blurred copy of the picture fills a vertical sequence", async () => {
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
      await createProject(app, page, "Vertical", work.dir);
      const name = "postcard.png";
      const file = path.join(work.media, name);
      writeFileSync(file, png(48, 32, ORANGE));
      await pickFiles(app, [file]);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(1);
      await dropCard(page, name, "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
      await editorCaughtUp(page);
      await showTimeline(page, 9);
      await movePlayhead(page, 1);

      // Nothing selected: the Sequence section. Make the sequence vertical, 1080 × 1920.
      const g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(7.5)), laneY(g, "V1"));
      const panel = page.getByTestId("sequence-inspector");
      await expect(panel).toBeVisible(BOUND);
      for (const [field, size] of [
        ["Width", "1080"],
        ["Height", "1920"],
      ] as const) {
        await panel.getByRole("spinbutton", { name: field }).dblclick();
        await page.keyboard.type(size);
        await page.keyboard.press("Enter");
        const key = field === "Width" ? "width" : "height";
        await expect.poll(async () => (await background(page))[key], BOUND).toBe(Number(size));
        await editorCaughtUp(page);
      }

      // The photo is a landscape band on black; Fill says the color only.
      const surface = page.locator(".cw-preview__surface");
      await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, BOUND);
      await expect.poll(() => look(page), { timeout: 30_000 }).toBe("photo");
      const fill = panel.getByLabel("Fill");
      await expect(fill.locator("option:checked")).toHaveText("Color only");
      await expect(panel.getByRole("slider", { name: "Blur" })).toBeHidden();
      await snapshot(page, info, "black bars");

      // Blurred picture: the whole frame is lit, the bars a dimmer copy of the photo's colour.
      await fill.selectOption({ label: "Blurred picture" });
      await expect
        .poll(async () => (await background(page)).background_fill, BOUND)
        .toEqual({ kind: "blur", source: "below", radius: 0.05, dim: 0.25 });
      await expect.poll(() => look(page), { timeout: 30_000 }).toBe("filled");
      const filled = await seen(page);
      if (filled === null) throw new Error("the preview went dark");
      expect(filled.top[0]).toBeLessThan(filled.middle[0] - 8);
      expect(filled.top[0]).toBeGreaterThan(120);
      expect(filled.top[0]).toBeGreaterThan(filled.top[2] + 60);
      const blur = panel.getByRole("slider", { name: "Blur" });
      const dim = panel.getByRole("slider", { name: "Dim" });
      await expect(blur).toHaveAttribute("aria-valuetext", "5%");
      await expect(dim).toHaveAttribute("aria-valuetext", "25%");
      await snapshot(page, info, "blurred fill");

      // Dim all the way (End on the slider): the bars are black again, the photo is not.
      await editorCaughtUp(page);
      await dim.focus();
      await page.keyboard.press("End");
      await expect.poll(async () => (await background(page)).background_fill?.dim, BOUND).toBe(1);
      await expect.poll(() => look(page), { timeout: 30_000 }).toBe("photo");

      // Undo names what it takes back, and the blurred bars return.
      await editorCaughtUp(page);
      const undo = page.getByRole("button", { name: /^Undo/ });
      await expect(undo).toHaveAccessibleName(/Change sequence background dim/);
      await undo.click();
      await expect
        .poll(async () => (await background(page)).background_fill?.dim, BOUND)
        .toBe(0.25);
      await expect.poll(() => look(page), { timeout: 30_000 }).toBe("filled");

      // A wider blur is one more step; the frame stays filled.
      await blur.focus();
      await page.keyboard.press("ArrowRight");
      await expect
        .poll(async () => (await background(page)).background_fill?.radius ?? 0, BOUND)
        .toBeGreaterThan(0.05);
      await expect.poll(() => look(page), { timeout: 30_000 }).toBe("filled");

      // Color only: the fill is gone from the document and the bars are black.
      await editorCaughtUp(page);
      await fill.selectOption({ label: "Color only" });
      await expect
        .poll(async () => (await background(page)).background_fill ?? null, BOUND)
        .toBeNull();
      await expect.poll(() => look(page), { timeout: 30_000 }).toBe("photo");
      await expect(panel.getByRole("slider", { name: "Blur" })).toBeHidden();
    } finally {
      await closeApp();
      work.remove();
    }
  });
});
