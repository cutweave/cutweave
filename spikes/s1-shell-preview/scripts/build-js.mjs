// SPDX-License-Identifier: GPL-3.0-or-later
// Bundle main, preload and renderer with esbuild into desktop/dist/.
import { copyFileSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const src = join(root, "desktop", "src");
const out = join(root, "desktop", "dist");
rmSync(out, { recursive: true, force: true });
mkdirSync(join(out, "renderer"), { recursive: true });

const common = { bundle: true, sourcemap: "linked", target: "es2023", logLevel: "info" };

await build({
  ...common,
  entryPoints: { main: join(src, "main.ts"), preload: join(src, "preload.ts") },
  outdir: out,
  outExtension: { ".js": ".cjs" },
  platform: "node",
  format: "cjs",
  external: ["electron"],
});

await build({
  ...common,
  entryPoints: [join(src, "renderer", "main.ts")],
  outfile: join(out, "renderer", "main.js"),
  platform: "browser",
  format: "esm",
});

for (const f of ["index.html", "sandboxed.html", "style.css"]) {
  copyFileSync(join(src, "renderer", f), join(out, "renderer", f));
}
