// SPDX-License-Identifier: GPL-3.0-or-later
// F55 Slideshow and frame to the bin (docs/user-flows.md; WP-2F.05d, ADR-0035).
//
// Three pictures become a slideshow: the bin's menu offers "Import as slideshow…", the (stubbed)
// OS picker answers with the pictures, the app's own dialog asks for the seconds per picture and
// the order, and three clips of that length appear one after the other, with three cards in the
// bin. One undo takes all of it back; redo brings it back.
//
// The frame the Preview shows becomes a picture: right-click the Preview, "Add frame to media
// bin", and a new image card appears, named after the sequence and the timecode. One undo
// removes it. The command palette runs the same command.
//
// The pictures are written by the flow (small PNG files of one colour each): no media is
// committed. Needs an engine built with FFmpeg (`just e2e-flows` builds it when the bundle is
// set up), because the frame is rendered and read back as a real picture; without it the flow
// skips and says why.
import { writeFileSync } from "node:fs";
import path from "node:path";
import { deflateSync } from "node:zlib";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  createProject,
  editorCaughtUp,
  hasRealEngine,
  launchFlowApp,
  mediaCard,
  mediaCards,
  pickFiles,
  sec,
  sequence,
  shortcut,
  snapshot,
  timecode,
  toast,
  workspace,
} from "./helpers.ts";

/** CRC-32 of `bytes` (PNG chunks carry one). */
function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/** A 64 × 36 PNG file filled with one colour. */
function png([r, g, b]: readonly [number, number, number]): Buffer {
  const [width, height] = [64, 36];
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
  header.set([8, 2, 0, 0, 0], 8); // 8 bits per sample, RGB
  const row = Buffer.concat([Buffer.from([0]), Buffer.alloc(width * 3)]);
  for (let x = 0; x < width; x += 1) row.set([r, g, b], 1 + x * 3);
  const rows = Buffer.concat(new Array<Buffer>(height).fill(row));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(rows)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** Every clip of the sequence as `[name of its media, start, duration]`, by start time. */
async function clips(page: Page): Promise<[string, number, number][]> {
  const seq = await sequence(page);
  const listed = await editorCall<{ media: { id: string; name: string }[] }>(page, "media.list");
  const names = new Map(listed.media.map((m) => [m.id, m.name]));
  return seq.tracks
    .flatMap((t) => t.items)
    .sort((a, b) => a.start - b.start)
    .map((i) => [names.get(i.media) ?? "?", i.start, i.duration]);
}

test.describe("F55 slideshow and frame to the bin", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("three pictures become a slideshow; the Preview's frame becomes a picture; each undoes in one step", async () => {
    test.setTimeout(300_000);
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
      await createProject(app, page, "Slides", work.dir);

      const pictures: [string, readonly [number, number, number]][] = [
        ["shot10.png", [30, 30, 200]],
        ["shot2.png", [30, 200, 30]],
        ["shot1.png", [200, 30, 30]],
      ];
      const files = pictures.map(([name, colour]) => {
        const file = path.join(work.media, name);
        writeFileSync(file, png(colour));
        return file;
      });

      // The bin's menu: Import as slideshow… (the OS picker answers with the three pictures).
      await pickFiles(app, files);
      await page.locator(".cw-media .cw-empty").click({ button: "right" });
      const binMenu = page.getByRole("menu", { name: "Media bin actions" });
      await expect(binMenu).toBeVisible();
      await binMenu.getByRole("menuitem", { name: /^Import as slideshow…/ }).click();

      // The app's dialog: two seconds per picture, by number (shot1, shot2, shot10).
      const dialog = page.getByRole("dialog", { name: "Import as slideshow" });
      await expect(dialog).toBeVisible();
      await expect(dialog).toContainText("3 pictures");
      const seconds = dialog.getByLabel("Seconds per picture");
      await seconds.fill("2");
      await dialog.getByRole("radio", { name: "By number" }).click();
      await snapshot(page, info, "slideshow dialog");
      await dialog.getByRole("button", { name: "Import", exact: true }).click();
      await expect(dialog).toBeHidden();

      // Three cards, and three clips of two seconds one after the other.
      await expect(toast(page, "Slideshow imported").first()).toContainText(
        "3 pictures are on the timeline.",
      );
      await expect(mediaCards(page)).toHaveCount(3);
      await editorCaughtUp(page);
      const placed: [string, number, number][] = [
        ["shot1.png", 0, sec(2)],
        ["shot2.png", sec(2), sec(2)],
        ["shot10.png", sec(4), sec(2)],
      ];
      await expect.poll(() => clips(page)).toEqual(placed);
      await snapshot(page, info, "slideshow on the timeline");

      // One undo removes the clips and the cards; redo brings all of it back.
      await shortcut(app, "Control+Z");
      await expect(mediaCards(page)).toHaveCount(0);
      await expect.poll(() => clips(page)).toEqual([]);
      await shortcut(app, "Control+Shift+Z");
      await expect(mediaCards(page)).toHaveCount(3);
      await expect.poll(() => clips(page)).toEqual(placed);
      await editorCaughtUp(page);

      // The Preview's menu: Add frame to media bin. The playhead is at the start.
      await expect(timecode(page)).toHaveText("00:00:00:00");
      const surface = page.locator(".cw-preview__surface");
      await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 20_000 });
      await page.locator("canvas.cw-preview__canvas").click({ button: "right" });
      const previewMenu = page.getByRole("menu", { name: "Preview actions" });
      await expect(previewMenu).toBeVisible();
      await previewMenu.getByRole("menuitem", { name: /^Add frame to media bin/ }).click();
      await expect(toast(page, "Frame added to the media bin").first()).toBeVisible({
        timeout: 30_000,
      });

      // A new image card, named after the sequence and the timecode.
      await expect(mediaCards(page)).toHaveCount(4);
      const frameName = /00:00:00:00$/;
      await expect(mediaCard(page, "00:00:00:00")).toBeVisible();
      const listed = await editorCall<{ media: { name: string; kind: string; path: string }[] }>(
        page,
        "media.list",
      );
      const frame = listed.media.find((m) => frameName.test(m.name));
      expect(frame?.kind).toBe("image");
      expect(frame?.path.replaceAll("\\", "/")).toMatch(
        /\/Slides\/generated\/frames\/med_\w+\.png$/,
      );
      await snapshot(page, info, "frame in the bin");

      // One undo removes the card (and nothing else).
      await shortcut(app, "Control+Z");
      await expect(mediaCards(page)).toHaveCount(3);
      await expect.poll(() => clips(page)).toEqual(placed);
      await editorCaughtUp(page);

      // The command palette runs the same command.
      const palette = page.getByRole("dialog", { name: "Command palette" });
      await shortcut(app, "Control+K");
      await expect(palette).toBeVisible();
      await expect(palette.getByRole("combobox", { name: "Search commands" })).toBeFocused();
      await page.keyboard.type("add frame to media bin");
      await expect(palette.getByRole("option").first()).toContainText("Add frame to media bin");
      await page.keyboard.press("Enter");
      await expect(palette).toHaveCount(0);
      await expect(mediaCards(page)).toHaveCount(4, { timeout: 30_000 });
    } finally {
      await closeApp();
      work.remove();
    }
  });
});
