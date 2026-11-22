// SPDX-License-Identifier: GPL-3.0-or-later
// Bundle the transport (b) entry points (WP-0C.06) into desktop/dist/ after build-js.mjs:
//   utility.cjs        utilityProcess frame pump (loads the napi addon)
//   preload-b.cjs      sandboxed preload (MessagePort handoff only)
//   renderer/b/        sandboxed page (main.js), its draw=worker worker (worker.js), html, css
//   preload-texture.cjs + renderer/b/{texture,producer}.*   sharedTexture probe (--transport=texture)
// main-b.ts is imported by main.ts and bundled into main.cjs by build-js.mjs.
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const src = join(root, "desktop", "src", "transport-b");
const out = join(root, "desktop", "dist");
mkdirSync(join(out, "renderer", "b"), { recursive: true });

const common = { bundle: true, sourcemap: "linked", target: "es2023", logLevel: "info" };

await build({
  ...common,
  entryPoints: {
    utility: join(src, "utility.ts"),
    "preload-b": join(src, "preload.ts"),
    "preload-texture": join(src, "preload-texture.ts"),
  },
  outdir: out,
  outExtension: { ".js": ".cjs" },
  platform: "node",
  format: "cjs",
  external: ["electron"],
});

await build({
  ...common,
  entryPoints: {
    main: join(src, "renderer.ts"),
    worker: join(src, "worker.ts"),
    texture: join(src, "texture.ts"),
    producer: join(src, "producer.ts"),
  },
  outdir: join(out, "renderer", "b"),
  platform: "browser",
  format: "esm",
});

for (const f of ["index.html", "style.css", "texture.html", "producer.html", "producer.css"]) {
  copyFileSync(join(src, f), join(out, "renderer", "b", f));
}
