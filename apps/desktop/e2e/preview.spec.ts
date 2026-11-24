// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1M.04 e2e: the preview panel end to end. The fake engine stands in for the playback
// subsystem (WP-1I.05 wires the real one): it runs the `view.*` transport, broadcasts
// `playback.state`, publishes frames through the engine's real `FramePublisher` (the Rust
// `ring_fixture`) and serves still frames on `/media/frame`. The editor, the keymap, the
// preload bridge, the WebGPU presenter and the JPEG fallback are the real ones.
//
// The GPU path runs wherever this machine has a WebGPU adapter; without one the editor falls back
// and these tests assert the fallback instead (Spike S1 allows it). `CUTWEAVE_PREVIEW=jpeg`
// forces the fallback, as CI machines without a GPU do.

import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import {
  desktopDir,
  editorHandle,
  editorState,
  expect,
  type LaunchedApp,
  launchApp,
  test,
} from "./fixtures.ts";
import { fixtureExe } from "./ring-fixture.ts";

const PREVIEW_ENV = { FAKE_ENGINE_PROJECT: "three-clips", FAKE_ENGINE_RING: fixtureExe };

const started = Date.now();
const log = (message: string) =>
  console.log(`[preview e2e +${((Date.now() - started) / 1000).toFixed(1)}s] ${message}`);

/** Launches with the three-clip project and waits until the editor has it and a preview path. */
async function launchPreview(
  env: Record<string, string> = {},
  switches: string[] = [],
): Promise<{ launched: LaunchedApp; page: Page; mode: string; reason: string | null }> {
  const launched = await launchApp({ ...PREVIEW_ENV, ...env }, switches);
  const { page } = launched;
  log("launched");
  await editorHandle(page);
  await expect.poll(async () => (await editorState(page)).items, { timeout: 20_000 }).toBe(3);
  log("three clips loaded");
  const surface = page.locator(".cw-preview__surface");
  await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 20_000 });
  // Frames reach the canvas: the GPU path presents from the ring, or the JPEG path drew one.
  await expect(surface).toHaveAttribute("data-live", "true", { timeout: 20_000 });
  log(`preview path ${await surface.getAttribute("data-path")}, live`);
  return {
    launched,
    page,
    mode: (await surface.getAttribute("data-path")) ?? "",
    reason: await surface.getAttribute("data-reason"),
  };
}

/**
 * Samples the preview for `ms`: a hash of the composited pixels of a 64 × 36 region at the
 * canvas's centre, captured as screenshots (what is on screen, whichever path drew it; reading a
 * WebGPU canvas back from script is not reliable). Away from the overlay's timecode readout.
 */
async function sampleHashes(page: Page, ms: number): Promise<string[]> {
  const box = await page.locator("canvas.cw-preview__canvas").boundingBox();
  if (box === null) throw new Error("no preview canvas");
  const clip = {
    x: Math.round(box.x + box.width / 2 - 32),
    y: Math.round(box.y + box.height / 2 - 18),
    width: 64,
    height: 36,
  };
  const out: string[] = [];
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const png = await page.screenshot({ clip, animations: "allow", caret: "initial" });
    out.push(createHash("sha1").update(png).digest("hex"));
  }
  return out;
}

const changes = (hashes: string[]): number =>
  hashes.reduce((n, h, i) => (i > 0 && h !== hashes[i - 1] ? n + 1 : n), 0);

/** `HH:MM:SS:FF` → frames at 30 fps. */
function frames(timecode: string | null): number {
  const [hh, mm, ss, ff] = (timecode ?? "").split(/[:;]/).map(Number);
  return (((hh ?? 0) * 60 + (mm ?? 0)) * 60 + (ss ?? 0)) * 30 + (ff ?? 0);
}

async function playPauseChecks(page: Page): Promise<void> {
  const timecode = page.locator(".cw-transport__time");
  const play = page.getByRole("button", { name: "Play", exact: true });
  await expect(play).toBeEnabled();
  const before = frames(await timecode.textContent());
  log("play enabled");

  // Keys go to the page, not to a focused control.
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press("Space");
  await expect(page.getByRole("button", { name: "Pause", exact: true })).toBeVisible();
  log("playing");
  const playing = await sampleHashes(page, 2000);
  const moved = changes(playing);
  log(`${playing.length} samples in 2 s, ${moved} changes while playing`);
  log(
    `present stats ${JSON.stringify(await page.evaluate(() => window.cutweave.preview.getPresentStats()))}`,
  );
  expect(moved).toBeGreaterThanOrEqual(10);
  const during = frames(await timecode.textContent());
  expect(during - before).toBeGreaterThanOrEqual(30);

  await page.keyboard.press("Space");
  await expect(play).toBeVisible();
  // Let the frames already on their way land, then nothing may change.
  await page.waitForTimeout(400);
  const stopped = frames(await timecode.textContent());
  const paused = await sampleHashes(page, 1000);
  expect(changes(paused)).toBe(0);
  expect(frames(await timecode.textContent())).toBe(stopped);
}

test.describe("preview panel", () => {
  test("Space plays: the canvas changes and the timecode advances; Space again pauses", async () => {
    const { launched, page, mode, reason } = await launchPreview();
    try {
      console.log(`[preview e2e] path: ${mode}${reason ? ` (${reason})` : ""}`);
      if (mode === "jpeg") {
        // No WebGPU adapter here: the reduced path is the one under test (S1 allows it).
        expect(reason).toBe("webgpu");
        await expect(page.locator(".cw-preview__banner")).toContainText("Reduced preview");
      } else {
        await expect(page.locator(".cw-preview__banner")).toHaveCount(0);
      }
      await playPauseChecks(page);
      if (mode === "gpu") {
        const stats = await page.evaluate(() => window.cutweave.preview.getPresentStats());
        console.log(`[preview e2e] present stats: ${JSON.stringify(stats)}`);
        expect(stats).toMatchObject({
          presenter: "webgpu",
          presenterErrors: 0,
          presenterLost: false,
        });
        expect(stats?.framesPresented).toBeGreaterThan(30);
      }
    } finally {
      await launched.close();
    }
  });

  test("CUTWEAVE_PREVIEW=jpeg forces still frames from /media/frame, with a banner", async () => {
    const { launched, page, mode, reason } = await launchPreview({ CUTWEAVE_PREVIEW: "jpeg" });
    try {
      expect([mode, reason]).toEqual(["jpeg", "forced"]);
      await expect(page.locator(".cw-preview__banner")).toContainText("CUTWEAVE_PREVIEW=jpeg");
      await expect(page.locator("canvas.cw-preview__canvas")).toHaveAttribute("data-path", "jpeg");
      await playPauseChecks(page);
    } finally {
      await launched.close();
    }
  });

  for (const scale of [1, 1.5]) {
    test(`resize at DPR ${scale}: backing store = CSS size × DPR, overlay aligned, engine size follows`, async () => {
      const { launched, page, mode } = await launchPreview({}, [
        `--force-device-scale-factor=${scale}`,
      ]);
      try {
        const { app } = launched;
        // Turn the safe areas on so the overlay has drawn content to align.
        await page.getByRole("button", { name: "Safe areas" }).click();
        const sizes: [number, number][] = [
          [1280, 800],
          [1000, 680],
          [1500, 940],
          [1100, 900],
          [1366, 720],
        ];
        for (const [w, h] of sizes) {
          await app.evaluate(
            ({ BrowserWindow }, [width, height]) => {
              BrowserWindow.getAllWindows()[0]?.setContentSize(width as number, height as number);
            },
            [w, h],
          );
          await expect
            .poll(
              () =>
                page.evaluate(() => {
                  const canvas = document.querySelector<HTMLCanvasElement>(
                    "canvas.cw-preview__canvas",
                  );
                  const overlay = document.querySelector(".cw-preview__overlay");
                  if (canvas === null || overlay === null) return "missing";
                  const c = canvas.getBoundingClientRect();
                  const o = overlay.getBoundingClientRect();
                  const dpr = window.devicePixelRatio;
                  const backing =
                    Math.abs(canvas.width - c.width * dpr) <= 1 &&
                    Math.abs(canvas.height - c.height * dpr) <= 1;
                  const aligned =
                    Math.abs(o.left - c.left) < 0.5 &&
                    Math.abs(o.top - c.top) < 0.5 &&
                    Math.abs(o.width - c.width) < 0.5 &&
                    Math.abs(o.height - c.height) < 0.5;
                  return backing && aligned
                    ? "ok"
                    : `canvas ${canvas.width}x${canvas.height} css ${c.width}x${c.height} dpr ${dpr} overlay ${o.left},${o.top} ${o.width}x${o.height}`;
                }),
              { message: `window ${w}x${h}` },
            )
            .toBe("ok");
          // The sequence's 16:9 shape is kept inside the stage.
          const ratio = await page.evaluate(() => {
            const r = document.querySelector("canvas.cw-preview__canvas")?.getBoundingClientRect();
            return r === undefined ? 0 : r.width / r.height;
          });
          expect(Math.abs(ratio - 16 / 9)).toBeLessThan(0.01);
          if (mode === "gpu") {
            // The engine renders at the canvas size (rounded to even) after the 50 ms debounce.
            await expect
              .poll(() =>
                page.evaluate(() => {
                  const canvas = document.querySelector<HTMLCanvasElement>(
                    "canvas.cw-preview__canvas",
                  );
                  const even = (v: number) => Math.max(2, Math.round(Math.floor(v) / 2) * 2);
                  try {
                    const s = window.cutweave.preview.getStatus();
                    return (
                      canvas !== null &&
                      s.previewWidth === even(canvas.width) &&
                      s.previewHeight === even(canvas.height)
                    );
                  } catch {
                    // Between rings (it moved to a larger one): ask again.
                    return false;
                  }
                }),
              )
              .toBe(true);
          }
        }
        // The guides sit inside the canvas box.
        const inside = await page.evaluate(() => {
          const c = document.querySelector("canvas.cw-preview__canvas")?.getBoundingClientRect();
          const g = document
            .querySelector('.cw-preview__safe[data-area="title"]')
            ?.getBoundingClientRect();
          if (c === undefined || g === undefined) return false;
          return g.left > c.left && g.right < c.right && g.top > c.top && g.bottom < c.bottom;
        });
        expect(inside).toBe(true);
      } finally {
        await launched.close();
      }
    });
  }

  // WP-1M.04's 60 s playback figure (recorded, non-gating): run with CUTWEAVE_PREVIEW_BENCH=1.
  test("60 s of 1080p30 playback: presenter instrumentation", async () => {
    test.skip(process.env.CUTWEAVE_PREVIEW_BENCH !== "1", "set CUTWEAVE_PREVIEW_BENCH=1 to run");
    test.setTimeout(150_000);
    const { launched, page, mode } = await launchPreview({
      FAKE_ENGINE_RING_SIZE: "1920x1080",
      FAKE_ENGINE_RING_FPS: "30",
      FAKE_ENGINE_RING_PIN: "1",
    });
    try {
      test.skip(mode !== "gpu", "needs the GPU path");
      await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
      await page.keyboard.press("Space");
      await page.waitForTimeout(2000);
      const start = await page.evaluate(() => window.cutweave.preview.getPresentStats());
      const tickFps: number[] = [];
      for (let s = 0; s < 60; s++) {
        await page.waitForTimeout(1000);
        const now = await page.evaluate(() => window.cutweave.preview.getPresentStats());
        tickFps.push(now?.tickFps ?? 0);
      }
      const end = await page.evaluate(() => window.cutweave.preview.getPresentStats());
      if (start === null || end === null) throw new Error("no present stats");
      const result = {
        what: "WP-1M.04 preview: 60 s playback, 1080p30 RGBA8 ring frames (ring_fixture), GPU path",
        seconds: 60,
        avgTickFps: (end.ticks - start.ticks) / 60,
        minTickFps: Math.min(...tickFps),
        framesPresented: end.framesPresented - start.framesPresented,
        avgPresentFps: (end.framesPresented - start.framesPresented) / 60,
        droppedFrames: end.seqGaps - start.seqGaps,
        longTicks: end.longTicks - start.longTicks,
        presenterErrors: end.presenterErrors,
        lastFrame: `${end.lastWidth}x${end.lastHeight} ${end.lastFormat}`,
      };
      console.log(`[preview bench] ${JSON.stringify(result)}`);
      const out = path.join(desktopDir, "..", "..", "test-results", "desktop");
      mkdirSync(out, { recursive: true });
      writeFileSync(path.join(out, "preview-bench.json"), `${JSON.stringify(result, null, 2)}\n`);
    } finally {
      await launched.close();
    }
  });
});
