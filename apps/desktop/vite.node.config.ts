// SPDX-License-Identifier: GPL-3.0-or-later
import { builtinModules } from "node:module";
import { defineConfig } from "vite";

// Main and preload are Node-side CommonJS bundles (Electron loads the preload with `require`,
// and a non-sandboxed preload may `require` the native bridge addon, ADR-001). The renderer is
// built from `packages/editor` by `build:renderer`.
export default defineConfig({
  build: {
    outDir: "dist",
    emptyOutDir: false,
    target: "node24",
    minify: false,
    sourcemap: true,
    lib: {
      entry: { "main/index": "src/main/index.ts", "preload/index": "src/preload/index.ts" },
      formats: ["cjs"],
      fileName: (_format, name) => `${name}.cjs`,
    },
    rolldownOptions: {
      external: ["electron", ...builtinModules, ...builtinModules.map((m) => `node:${m}`)],
    },
  },
});
