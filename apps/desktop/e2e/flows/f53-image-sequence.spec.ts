// SPDX-License-Identifier: GPL-3.0-or-later
// F53 Image sequences (docs/user-flows.md; WP-2F.05c, ADR-0035): a numbered run of pictures is
// chosen in the bin's Import dialog; the app asks "Import as an image sequence?"; the answer
// makes one card (an image sequence, not twelve stills) that drops on the timeline as one clip
// as long as its pictures (12 at 30 fps), and stepping the playhead shows each frame's own
// picture: read from the pixels on screen. Settings > Media and import has the frame rate a new
// sequence gets; Undo takes the sequence out of the bin again.
//
// The pictures are written by the flow (flat-colour PNGs, the frame number in the colour): no
// media is committed. Needs an engine built with FFmpeg (`just e2e-flows` builds it when the
// bundle is set up); without it the flow skips and says why.
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { deflateSync } from "node:zlib";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  closeSettings,
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  openSettingsPage,
  pickFiles,
  snapshot,
  timecode,
  toast,
  workspace,
} from "./helpers.ts";

const FRAMES = 12;
/** Six levels per channel, 44 apart: a colour survives the compositor and the screen. */
const LEVELS = [20, 64, 108, 152, 196, 240] as const;

type Rgb = readonly [number, number, number];

/** The colour that carries `number` (0 to 215): one base-6 digit per channel. */
const colourOf = (number: number): Rgb => [
  LEVELS[number % 6] ?? 0,
  LEVELS[Math.floor(number / 6) % 6] ?? 0,
  LEVELS[Math.floor(number / 36) % 6] ?? 0,
];

/** The number a colour on screen carries (the nearest level of each channel). */
function numberOf([r, g, b]: Rgb): number {
  const digit = (v: number) =>
    LEVELS.reduce(
      (best, level, i) => (Math.abs(level - v) < Math.abs((LEVELS[best] ?? 0) - v) ? i : best),
      0,
    );
  return digit(r) + 6 * digit(g) + 36 * digit(b);
}

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
  // Every row: filter type 0, then the pixels.
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

/** `HH:MM:SS:FF` → frames at 30 fps (the new project's rate). */
async function timecodeFrames(page: Page): Promise<number> {
  const [hh, mm, ss, ff] = ((await timecode(page).textContent()) ?? "").split(/[:;]/).map(Number);
  return (((hh ?? 0) * 60 + (mm ?? 0)) * 60 + (ss ?? 0)) * 30 + (ff ?? 0);
}

/**
 * The number of the picture the person sees in the preview: the colour a third of the way in
 * from the canvas's top left corner (away from the overlay's readouts), read from a screenshot.
 */
async function shownNumber(page: Page): Promise<number> {
  const box = await page.locator("canvas.cw-preview__canvas").boundingBox();
  if (box === null) return -1;
  const shot = await page.screenshot({
    clip: {
      x: Math.round(box.x + box.width / 3),
      y: Math.round(box.y + box.height / 3),
      width: 4,
      height: 4,
    },
    animations: "allow",
    caret: "initial",
  });
  const colour = await page.evaluate(async (base64) => {
    const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const ctx = canvas.getContext("2d");
    if (ctx === null) throw new Error("no 2D context");
    ctx.drawImage(bitmap, 0, 0);
    const { data } = ctx.getImageData(1, 1, 1, 1);
    return [data[0] ?? 0, data[1] ?? 0, data[2] ?? 0] as const;
  }, shot.toString("base64"));
  return numberOf(colour);
}

async function press(page: Page, key: string): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press(key);
}

test.describe("F53 image sequences", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a numbered run of pictures imports as one item that plays like video", async () => {
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
      await createProject(app, page, "Sequence", work.dir);

      // Settings > Media and import: a new sequence gets the frame rate of the sequence unless
      // the person picks one.
      const settings = await openSettingsPage(page, app, "Media and import");
      const rate = settings.getByRole("combobox", { name: "Frame rate for image sequences" });
      await expect(rate).toHaveValue("sequence");
      await closeSettings(settings);

      // Twelve pictures, shot_0001.png to shot_0012.png: picture n carries the number n.
      const render = path.join(work.media, "render");
      mkdirSync(render);
      const files = Array.from({ length: FRAMES }, (_, i) => {
        const file = path.join(render, `shot_${String(i + 1).padStart(4, "0")}.png`);
        writeFileSync(file, png(64, 36, colourOf(i + 1)));
        return file;
      });

      // Import… with the run chosen: the app asks before it imports anything.
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      const offer = page.getByRole("dialog", { name: "Import as an image sequence?" });
      await expect(offer).toBeVisible();
      await expect(offer).toContainText(
        "You chose 12 numbered pictures, shot_0001.png to shot_0012.png.",
      );
      await expect(mediaCards(page)).toHaveCount(0);
      await snapshot(page, info, "the offer");
      await offer.getByRole("button", { name: "Import as image sequence" }).click();
      await expect(offer).toBeHidden();

      // One card for the whole run: an image sequence, named after its first picture.
      await expect(toast(page, "Image sequence imported")).toContainText("12 pictures at 30 fps.");
      await expect(mediaCards(page)).toHaveCount(1);
      const card = mediaCard(page, "shot_0001.png");
      await expect(card).toBeVisible();
      await expect(card).toContainText("Image sequence");
      const listed = await editorCall<{ media: { kind: string }[] }>(page, "media.list");
      expect(listed.media.map((m) => m.kind)).toEqual(["image_sequence"]);

      // On the timeline it is one clip as long as its pictures: 12 frames at 30 fps.
      await dropCard(page, "shot_0001.png", "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
      await editorCaughtUp(page);
      const clip = (await layout(page))[0]?.clips[0];
      expect((clip?.[1] ?? 0) - (clip?.[0] ?? 0)).toBeCloseTo(FRAMES / 30, 3);

      // Every frame shows its own picture: at the start the first one, then one more per step.
      const surface = page.locator(".cw-preview__surface");
      await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 20_000 });
      await press(page, "Home");
      await expect.poll(() => timecodeFrames(page)).toBe(0);
      await expect
        .poll(() => shownNumber(page), { timeout: 30_000, message: "the first picture" })
        .toBe(1);
      for (let frame = 1; frame < FRAMES; frame += 1) {
        await press(page, "ArrowRight");
        await expect.poll(() => timecodeFrames(page)).toBe(frame);
        await expect
          .poll(() => shownNumber(page), { timeout: 15_000, message: `frame ${frame}` })
          .toBe(frame + 1);
      }
      // And backwards: the same pictures again.
      for (const frame of [7, 6, 5]) {
        if (frame === 7) for (let i = 0; i < FRAMES - 1 - 7; i += 1) await press(page, "ArrowLeft");
        else await press(page, "ArrowLeft");
        await expect.poll(() => timecodeFrames(page)).toBe(frame);
        await expect
          .poll(() => shownNumber(page), { timeout: 15_000, message: `back to frame ${frame}` })
          .toBe(frame + 1);
      }
      await snapshot(page, info, "a frame of the sequence");

      // Undo twice (the clip, then the import): the bin is empty again; Redo brings it back.
      await chooseMenu(page, app, "Edit", /^Undo/);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(0);
      await chooseMenu(page, app, "Edit", /^Undo/);
      await expect(mediaCards(page)).toHaveCount(0);
      await chooseMenu(page, app, "Edit", /^Redo/);
      await expect(mediaCards(page)).toHaveCount(1);
    } finally {
      await closeApp();
      work.remove();
    }
  });
});
