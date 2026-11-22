// SPDX-License-Identifier: GPL-3.0-or-later
// Headless benchmark: builds the app, serves it with `vite preview`, drives it with
// Playwright's Chromium and prints the metrics JSON. Usage:
//   pnpm bench [--seconds 30] [--width 1600] [--height 900] [--uncapped] [--out results/x.json]
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build, preview } from "vite";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const opt = (name, dflt) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : dflt;
};
const seconds = Number(opt("seconds", 30));
const width = Number(opt("width", 1600));
const height = Number(opt("height", 900));
const uncapped = args.includes("--uncapped");
const out = opt("out", null);

let chromium;
try {
  ({ chromium } = await import("playwright"));
} catch (e) {
  console.error("playwright is not installed; run `pnpm install` in this directory.", e.message);
  process.exit(2);
}

await build({ root, logLevel: "warn" });
const server = await preview({ root, preview: { port: 0, strictPort: false, host: "127.0.0.1" } });
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
    "Could not launch Playwright Chromium (browser download blocked or missing?).\n" +
      "Run `pnpm exec playwright install chromium`, or use `pnpm dev` and read the HUD.\n" +
      e.message,
  );
  await server.close();
  process.exit(3);
}

try {
  const page = await browser.newPage({ viewport: { width, height } });
  page.on("pageerror", (e) => console.error("pageerror:", e.message));
  await page.goto(`${url}?bench=${seconds}&mode=${uncapped ? "uncapped" : "vsync"}`);
  await page.waitForFunction(() => window.__benchResult || window.__benchError, null, {
    timeout: (seconds + 60) * 1000,
  });
  const err = await page.evaluate(() => window.__benchError);
  if (err) throw new Error(err);
  const result = await page.evaluate(() => window.__benchResult);
  result.browser = `chromium ${browser.version()} (headless, playwright)`;
  result.platform = process.platform;
  const json = JSON.stringify(result, null, 2);
  console.log(json);
  if (out) {
    mkdirSync(dirname(resolve(root, out)), { recursive: true });
    writeFileSync(resolve(root, out), `${json}\n`);
  }
  const pass = result.fps.p5 >= 55 && result.hitTest.p95Ms < 2;
  console.error(`\nS1 criteria (fps p5 >= 55, hit-test p95 < 2 ms): ${pass ? "PASS" : "FAIL"}`);
} finally {
  await browser.close();
  await server.close();
}
