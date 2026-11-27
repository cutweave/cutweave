// SPDX-License-Identifier: GPL-3.0-or-later
// Headless timeline benchmark: builds bench/index.html with Vite, serves it, drives it with
// Playwright's Chromium and prints the metrics. Result JSON uses the bench result schema
// (bench/result.schema.json, WP-0I.03) so `cargo xtask bench-check` can consume it once budget
// rows exist for the `timeline.*` ids.
//
//   pnpm --filter @cutweave/timeline bench [--seconds 30] [--width 1600] [--height 900]
//       [--dpr 1] [--uncapped] [--lanes] [--min-fps 55] [--out bench/results/local.json]
//
// `--lanes` opens the keyframe lanes of every clip (WP-2C.05), the worst case for them.
// `--fixture <file>` runs on a real project's model and lanes instead of the synthetic one (the
// JSON `packages/editor/src/timeline/keyframes.p5k.test.ts` writes for P-5K).
//
// Gate: fps p5 >= --min-fps (default 55 on a dev machine, 45 when CI is set); vsync run only.
// Needs Playwright's Chromium (`pnpm exec playwright install chromium`).
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import { build, preview } from "vite";

const root = resolve(dirname(fileURLToPath(import.meta.url)));
const args = process.argv.slice(2);
const opt = (name, dflt) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : dflt;
};
const seconds = Number(opt("seconds", 30));
const width = Number(opt("width", 1600));
const height = Number(opt("height", 900));
const dpr = Number(opt("dpr", 1));
const uncapped = args.includes("--uncapped");
const media = args.includes("--no-media") ? 0 : 1;
const lanes = args.includes("--lanes") ? 1 : 0;
const fixture = opt("fixture", null);
const minFps = Number(opt("min-fps", process.env.CI ? 45 : 55));
const out = opt("out", null);

await build({
  root,
  logLevel: "warn",
  build: { outDir: resolve(root, "dist"), emptyOutDir: true },
});
if (fixture) copyFileSync(resolve(fixture), resolve(root, "dist", "fixture.json"));
const server = await preview({
  root,
  build: { outDir: resolve(root, "dist") },
  preview: { port: 0, host: "127.0.0.1" },
});
const url = server.resolvedUrls?.local?.[0];
if (!url) throw new Error("preview server did not report a URL");

let browser;
try {
  browser = await chromium.launch({
    headless: true,
    args: uncapped ? ["--disable-frame-rate-limit", "--disable-gpu-vsync"] : [],
  });
} catch (e) {
  console.error(
    `Could not launch Playwright Chromium (run \`pnpm exec playwright install chromium\`).\n${e.message}`,
  );
  await server.close();
  process.exit(3);
}

try {
  const page = await (
    await browser.newContext({ viewport: { width, height }, deviceScaleFactor: dpr })
  ).newPage();
  page.on("pageerror", (e) => console.error("pageerror:", e.message));
  await page.goto(
    `${url}?seconds=${seconds}&media=${media}&lanes=${lanes}&fixture=${fixture ? 1 : 0}`,
  );
  await page.waitForFunction(() => window.__benchResult || window.__benchError, null, {
    timeout: (seconds + 90) * 1000,
  });
  const err = await page.evaluate(() => window.__benchError);
  if (err) throw new Error(err);
  const r = await page.evaluate(() => window.__benchResult);
  r.browser = `chromium ${browser.version()} (headless, playwright)`;
  r.mode = uncapped ? "uncapped" : "vsync";
  r.platform = process.platform;
  console.log(JSON.stringify(r, null, 2));

  if (out) {
    let commit = "unknown";
    try {
      commit = execFileSync("git", ["rev-parse", "--short", "HEAD"], { cwd: root })
        .toString()
        .trim();
    } catch {}
    const suffix = uncapped ? "uncapped" : "vsync";
    const result = {
      machine: process.env.CUTWEAVE_BENCH_MACHINE ?? "local",
      commit,
      date: new Date().toISOString().slice(0, 10),
      suite: "timeline",
      toolchain: { rustc: null, ffmpeg: null, wgpu: null, electron: null },
      metrics: [
        {
          id: `timeline.pan_zoom_fps_p5.${suffix}`,
          value: r.fps.p5,
          unit: "fps",
          samples: r.frames,
        },
        {
          id: `timeline.frame_ms_p95.${suffix}`,
          value: r.frameMs.p95,
          unit: "ms",
          samples: r.frames,
        },
        {
          id: `timeline.draw_ms_p95.${suffix}`,
          value: r.drawMs.p95,
          unit: "ms",
          samples: r.frames,
        },
        {
          id: "timeline.hit_test_ms_p95",
          value: r.hitTest.p95,
          unit: "ms",
          samples: r.hitTest.calls,
        },
        { id: "timeline.model_build_ms", value: r.modelBuildMs, unit: "ms", samples: 1 },
      ],
      details: { viewport: r.viewport, dataset: r.dataset, browser: r.browser },
    };
    mkdirSync(dirname(resolve(out)), { recursive: true });
    writeFileSync(resolve(out), `${JSON.stringify(result, null, 2)}\n`);
  }

  // The gate applies to the vsync run; `--uncapped` only reports headroom.
  const pass = uncapped || r.fps.p5 >= minFps;
  console.error(
    `\nTimeline bench: fps p5 ${r.fps.p5} (gate >= ${uncapped ? "n/a, uncapped" : minFps}), draw p95 ${r.drawMs.p95} ms, ` +
      `hit-test p95 ${r.hitTest.p95} ms: ${pass ? "PASS" : "FAIL"}`,
  );
  if (!pass) process.exitCode = 1;
} finally {
  await browser.close();
  await server.close();
}
