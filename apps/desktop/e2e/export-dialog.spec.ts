// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1M.08, WP-2G.04 e2e: the export dialog, its progress, the top bar's chip, the toasts and the
// queue panel, against the fake engine (it ramps a job up over about two seconds and writes a small file; no FFmpeg).
// What a real export produces (ffprobe, Chromium playback) is the user flow F14
// (`e2e/flows/f14-export.spec.ts`), which needs the engine built with FFmpeg (WP-1I.05).

import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import type { Page } from "@playwright/test";
import { editorHandle, editorState, expect, launchApp, test } from "./fixtures.ts";
import { chooseMenu, pickSavePath, shortcut } from "./flows/helpers.ts";

const ENV = { FAKE_ENGINE_PROJECT: "three-clips", FAKE_ENGINE_ENCODERS: "h264_nvenc,h264_amf" };

async function open(env: Record<string, string> = {}) {
  const launched = await launchApp({ ...ENV, ...env });
  await editorHandle(launched.page);
  await expect
    .poll(async () => (await editorState(launched.page)).items, { timeout: 20_000 })
    .toBe(3);
  return launched;
}

const dialog = (page: Page) => page.getByRole("dialog", { name: "Export video" });
const progress = (page: Page) => page.getByRole("dialog", { name: /^Exporting / });
const toast = (page: Page, title: string) => page.locator(".cw-toast", { hasText: title });

/** Opens the dialog with the top bar's button and types the output path. */
async function openDialog(page: Page, file: string): Promise<void> {
  await page.getByRole("button", { name: "Export", exact: true }).click();
  await expect(dialog(page)).toBeVisible();
  await dialog(page).getByLabel("Save to").fill(file);
}

const startExport = (page: Page) =>
  dialog(page).getByRole("button", { name: "Export", exact: true }).click();

test.describe("export dialog (fake engine)", () => {
  test("shows the presets, the summary and the detected encoders, refuses bad values", async () => {
    const { app, page, close } = await open();
    try {
      await page.getByRole("button", { name: "Export", exact: true }).click();
      const d = dialog(page);
      await expect(d.getByLabel("Preset", { exact: true })).toHaveValue("h264-mp4-1080p");
      await expect(d.getByLabel("Save to")).toHaveValue(/\.mp4$/);
      await expect(d.getByTestId("export-summary")).toContainText(
        "1920 × 1080 · 30 fps · H.264 + AAC · MP4",
      );
      await expect(d.getByRole("radio", { name: "Whole sequence" })).toBeChecked();
      // A 9:16 preset on this 16:9 sequence says what happens.
      await d.getByLabel("Preset", { exact: true }).selectOption("tiktok");
      await expect(d.getByText(/^Fits with bars: TikTok is 9:16/)).toBeVisible();
      await d.getByLabel("Preset", { exact: true }).selectOption("h264-mp4-1080p");
      await d.getByRole("button", { name: "Advanced" }).click();
      await expect(d.getByLabel("Width")).toHaveValue("1920");
      await expect(d.getByLabel("Height")).toHaveValue("1080");
      await expect(d.getByLabel("Frame rate")).toHaveValue("30");
      await expect(d.getByRole("radio", { name: "Medium" })).toBeChecked();
      await expect(d.getByRole("switch", { name: "Use hardware encoder" })).toBeChecked();
      await expect(d.getByText("Found NVIDIA NVENC, AMD AMF. Faster than software.")).toBeVisible();
      await expect(d.getByRole("group", { name: "Encoder settings" })).toBeVisible();

      // Choose… asks the OS's save dialog, which starts at the proposed file, and takes its answer.
      const picked = path.join(tmpdir(), "picked.mp4");
      const proposed = await pickSavePath(app, picked);
      const first = await d.getByLabel("Save to").inputValue();
      await d.getByRole("button", { name: "Choose…" }).click();
      await expect(d.getByLabel("Save to")).toHaveValue(picked);
      expect(await proposed()).toEqual([first]);

      await d.getByLabel("Height").fill("1081");
      await startExport(page);
      await expect(d.getByRole("alert")).toHaveText(
        "Height must be an even number from 16 to 8192.",
      );
      await d.getByLabel("Height").fill("1080");
      await d.getByLabel("Save to").fill("relative.mp4");
      await startExport(page);
      await expect(d.getByRole("alert")).toHaveText(
        "Type the full path of the file, or use Choose…",
      );
      await d.getByRole("button", { name: "Cancel" }).click();
      await expect(d).toBeHidden();
    } finally {
      await close();
    }
  });

  test("Add to queue: the toast's Show queue opens the Exports panel with the export", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "cutweave-export-"));
    const file = path.join(dir, "queued.mp4");
    const { page, close } = await open();
    try {
      await openDialog(page, file);
      await dialog(page).getByRole("button", { name: "Add to queue" }).click();
      await expect(dialog(page)).toBeHidden();
      const queued = toast(page, "Added to the export queue");
      await expect(queued).toBeVisible();
      await queued.getByRole("button", { name: "Show queue" }).click();
      const panel = page.getByTestId("export-queue");
      await expect(panel).toBeVisible();
      await expect(panel.getByRole("listitem", { name: "queued.mp4, Done" })).toBeVisible({
        timeout: 15_000,
      });
      expect(existsSync(file)).toBe(true);
    } finally {
      await close();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("the progress goes on when the dialog is hidden; the chip brings it back; Cancel leaves no file", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "cutweave-export-"));
    const file = path.join(dir, "out.mp4");
    const { page, close } = await open();
    try {
      await openDialog(page, file);
      await dialog(page).getByRole("button", { name: "Advanced" }).click();
      await dialog(page).getByRole("radio", { name: "Low" }).click();
      await startExport(page);
      await expect(progress(page)).toBeVisible();
      await expect(
        progress(page).getByRole("progressbar", { name: "Export progress" }),
      ).toBeVisible();
      await expect(progress(page).getByText(/24\.5 fps · about 0:01 left/)).toBeVisible();
      await progress(page).getByRole("button", { name: "Hide" }).click();
      await expect(progress(page)).toBeHidden();
      const chip = page.getByTestId("export-chip");
      await expect(chip).toHaveText(/^Exporting \d+%$/);
      await chip.click();
      await expect(progress(page)).toBeVisible();
      await progress(page).getByRole("button", { name: "Cancel export" }).click();
      await expect(toast(page, "Export cancelled")).toBeVisible();
      await expect(progress(page)).toBeHidden();
      await expect(chip).toBeHidden();
      expect(existsSync(file)).toBe(false);
    } finally {
      await close();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("success: a toast with Reveal in folder, which asks the OS to show the file", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "cutweave-export-"));
    const file = path.join(dir, "out.mp4");
    const { app, page, close } = await open();
    try {
      await app.evaluate(({ shell }) => {
        const seen: string[] = [];
        (globalThis as unknown as { __revealed: string[] }).__revealed = seen;
        shell.showItemInFolder = (p: string) => {
          seen.push(p);
        };
      });
      await openDialog(page, file);
      await startExport(page);
      const done = toast(page, "Export complete");
      await expect(done).toBeVisible({ timeout: 15_000 });
      await expect(done).toContainText("out.mp4: exported 60 frames (2.00 s) with libx264");
      await expect(progress(page)).toBeHidden();
      expect(existsSync(file)).toBe(true);
      await done.getByRole("button", { name: "Reveal in folder" }).click();
      expect(
        await app.evaluate(() => (globalThis as unknown as { __revealed: string[] }).__revealed),
      ).toEqual([file]);
    } finally {
      await close();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a hardware encoder that fails restarts in software: the dialog says so", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "cutweave-export-"));
    const { page, close } = await open();
    try {
      await openDialog(page, path.join(dir, "restart.mp4"));
      await startExport(page);
      await expect(
        progress(page).getByText(
          "The hardware encoder failed, so the export started again in software.",
        ),
      ).toBeVisible({ timeout: 15_000 });
      await expect(toast(page, "Export complete")).toBeVisible({ timeout: 15_000 });
    } finally {
      await close();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("a failed export shows the engine's message and leaves no file", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "cutweave-export-"));
    const file = path.join(dir, "fail.mp4");
    const { page, close } = await open();
    try {
      await openDialog(page, file);
      await startExport(page);
      const failed = toast(page, "Export failed");
      await expect(failed).toBeVisible({ timeout: 15_000 });
      await expect(failed).toContainText("the encoder stopped: the disk is full");
      expect(existsSync(file)).toBe(false);
    } finally {
      await close();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("an existing file asks before it is replaced", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "cutweave-export-"));
    const file = path.join(dir, "there.mp4");
    writeFileSync(file, "old");
    const { page, close } = await open();
    try {
      await openDialog(page, file);
      await startExport(page);
      const ask = page.getByRole("dialog", { name: "Replace there.mp4?" });
      await expect(ask).toBeVisible();
      await ask.getByRole("button", { name: "Back" }).click();
      await expect(dialog(page).getByLabel("Save to")).toHaveValue(file);
      await startExport(page);
      await ask.getByRole("button", { name: "Replace" }).click();
      await expect(toast(page, "Export complete")).toBeVisible({ timeout: 15_000 });
    } finally {
      await close();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("File > Export… and Ctrl+M open it", async () => {
    const { app, page, close } = await open();
    try {
      await chooseMenu(page, app, "File", "Export…");
      await expect(dialog(page)).toBeVisible();
      await dialog(page).getByRole("button", { name: "Cancel" }).click();
      await expect(dialog(page)).toBeHidden();
      await shortcut(app, "Control+M");
      await expect(dialog(page)).toBeVisible();
      await dialog(page).getByRole("button", { name: "Cancel" }).click();
    } finally {
      await close();
    }
  });

  test("with an empty timeline Export is off, in the top bar, the menu and by key", async () => {
    const { app, page, close } = await launchApp({ ...ENV, FAKE_ENGINE_PROJECT: "" });
    try {
      await editorHandle(page);
      await expect.poll(async () => (await editorState(page)).connection).toBe("ready");
      await expect(page.getByRole("button", { name: "Export", exact: true })).toBeDisabled();
      await page.getByRole("menuitem", { name: "File", exact: true }).click();
      await expect(page.getByRole("menuitem", { name: /^Export…/ })).toBeDisabled();
      await page.keyboard.press("Escape");
      await shortcut(app, "Control+M");
      await expect(dialog(page)).toBeHidden();
    } finally {
      await close();
    }
  });
});
