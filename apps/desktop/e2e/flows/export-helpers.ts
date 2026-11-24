// SPDX-License-Identifier: GPL-3.0-or-later
// What F14 (export) needs besides the shared helpers: the corpus clips, `ffprobe`, and a plain
// Chromium page that plays the exported file. None of it changes the app under test.
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { _electron as electron } from "@playwright/test";
import { desktopDir } from "../fixtures.ts";

/**
 * The fetched corpus (`just corpus`): `CUTWEAVE_CORPUS`, else `testdata/corpus` of this checkout
 * or, in a git worktree, of the main checkout. `null` when it is not on this machine. Read only.
 */
export function corpusDir(): string | null {
  const candidates = [
    process.env.CUTWEAVE_CORPUS,
    path.join(desktopDir, "..", "..", "testdata", "corpus"),
  ];
  try {
    const common = execFileSync(
      "git",
      ["rev-parse", "--path-format=absolute", "--git-common-dir"],
      {
        cwd: desktopDir,
        encoding: "utf8",
      },
    ).trim();
    candidates.push(path.join(common, "..", "testdata", "corpus"));
  } catch {
    // Not a git checkout: the other candidates stand.
  }
  return (
    candidates.find(
      (c): c is string => c !== undefined && existsSync(path.join(c, "h264_1080p60.mp4")),
    ) ?? null
  );
}

/** Copies corpus clips into `dir` (the engine may read only the project's folders). */
export function copyCorpusClips(corpus: string, dir: string, names: readonly string[]): string[] {
  mkdirSync(dir, { recursive: true });
  return names.map((name) => {
    const target = path.join(dir, name);
    copyFileSync(path.join(corpus, name), target);
    return target;
  });
}

/** `ffprobe` from the FFmpeg bundle (`FFMPEG_DIR/bin`, set by scripts/ffmpeg/env.sh) or the PATH. */
export function ffprobePath(): string | null {
  const exe = process.platform === "win32" ? "ffprobe.exe" : "ffprobe";
  const fromBundle =
    process.env.FFMPEG_DIR === undefined ? null : path.join(process.env.FFMPEG_DIR, "bin", exe);
  if (fromBundle !== null && existsSync(fromBundle)) return fromBundle;
  const onPath = spawnSync(exe, ["-version"], { windowsHide: true });
  return onPath.status === 0 ? exe : null;
}

export interface Probed {
  readonly formatName: string;
  /** Container duration in seconds. */
  readonly duration: number;
  readonly video: { codec: string; width: number; height: number; frameRate: number } | null;
  readonly audio: { codec: string } | null;
}

/** What `ffprobe` says about a file; throws when it does not accept it. */
export function probe(ffprobe: string, file: string): Probed {
  const out = execFileSync(
    ffprobe,
    ["-v", "error", "-show_format", "-show_streams", "-of", "json", file],
    { encoding: "utf8", windowsHide: true },
  );
  const json = JSON.parse(out) as {
    format: { format_name: string; duration: string };
    streams: {
      codec_type: string;
      codec_name: string;
      width?: number;
      height?: number;
      r_frame_rate?: string;
    }[];
  };
  const video = json.streams.find((s) => s.codec_type === "video");
  const audio = json.streams.find((s) => s.codec_type === "audio");
  const [num, den] = (video?.r_frame_rate ?? "0/1").split("/").map(Number);
  return {
    formatName: json.format.format_name,
    duration: Number(json.format.duration),
    video:
      video === undefined
        ? null
        : {
            codec: video.codec_name,
            width: video.width ?? 0,
            height: video.height ?? 0,
            frameRate: (num ?? 0) / (den || 1),
          },
    audio: audio === undefined ? null : { codec: audio.codec_name },
  };
}

/**
 * Plays `file` in a `<video>` of a plain Chromium page (Electron's own, so H.264 and AAC decode)
 * and reports what the page saw: `readyState` once it can play, and the frame size. A separate
 * hidden window in a separate Electron process, never the app under test; it is closed before
 * this returns.
 */
export async function playInChromium(file: string): Promise<{
  readyState: number;
  videoWidth: number;
  videoHeight: number;
  duration: number;
  error: string | null;
}> {
  const dir = mkdtempSync(path.join(tmpdir(), "cutweave-flow-video-"));
  try {
    const main = path.join(dir, "main.cjs");
    writeFileSync(
      main,
      [
        'const { app, BrowserWindow } = require("electron");',
        "app.whenReady().then(() => {",
        "  const win = new BrowserWindow({ show: false, webPreferences: { sandbox: true } });",
        `  win.loadURL(${JSON.stringify(pathToFileURL(path.join(dir, "page.html")).href)});`,
        "});",
        'app.on("window-all-closed", () => app.quit());',
      ].join("\n"),
    );
    writeFileSync(
      path.join(dir, "page.html"),
      `<!doctype html><meta charset="utf-8"><title>probe</title>` +
        `<video id="v" muted preload="auto" src=${JSON.stringify(pathToFileURL(file).href)}></video>`,
    );
    const app = await electron.launch({ args: [main], cwd: dir });
    try {
      const page = await app.firstWindow();
      await page.waitForFunction(
        () => {
          const v = document.getElementById("v") as HTMLVideoElement;
          return v.readyState >= 3 || v.error !== null;
        },
        undefined,
        { timeout: 30_000 },
      );
      return await page.evaluate(() => {
        const v = document.getElementById("v") as HTMLVideoElement;
        return {
          readyState: v.readyState,
          videoWidth: v.videoWidth,
          videoHeight: v.videoHeight,
          duration: v.duration,
          error: v.error === null ? null : `code ${v.error.code}: ${v.error.message}`,
        };
      });
    } finally {
      await app.close();
    }
  } finally {
    rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  }
}
