// SPDX-License-Identifier: GPL-3.0-or-later
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";
import { ensureMathModule, STAGED_MODULE } from "../protocol/wasm/ensure.ts";

const uiSrc = fileURLToPath(new URL("../ui/src", import.meta.url));
const preferencesSrc = fileURLToPath(new URL("../preferences/src", import.meta.url));
const protocolSrc = fileURLToPath(new URL("../protocol/src", import.meta.url));
const timelineSrc = fileURLToPath(new URL("../timeline/src", import.meta.url));

const MATH_MODULE_ID = "virtual:cutweave-math-wasm";

/**
 * The shared math module (WP-2C.06, ADR-0033) as a script: `import bytes from
 * "virtual:cutweave-math-wasm"` is the module's bytes (a `Uint8Array`) inside a chunk from the app
 * origin, so the CSP needs `'wasm-unsafe-eval'` for compiling and nothing else (no `connect-src`
 * for the origin, no `data:` or `blob:`). Builds the module first when it is missing or stale, so
 * `just dev`, the e2e build and the packaged build all work on a fresh checkout.
 */
function mathModule(): Plugin {
  return {
    name: "cutweave-math-wasm",
    resolveId: (id) => (id === MATH_MODULE_ID ? `\0${MATH_MODULE_ID}` : null),
    load(id) {
      if (id !== `\0${MATH_MODULE_ID}`) return null;
      ensureMathModule();
      this.addWatchFile(STAGED_MODULE);
      const base64 = readFileSync(STAGED_MODULE).toString("base64");
      return `const text = atob("${base64}");
const bytes = new Uint8Array(text.length);
for (let i = 0; i < text.length; i += 1) bytes[i] = text.charCodeAt(i);
export default bytes;
`;
    },
  };
}

// Plain Vite app: `pnpm --filter @cutweave/editor dev` runs the editor with no engine.
// `@cutweave/ui`, `/protocol` and `/timeline` resolve to their source so the editor needs no prior build.
export default defineConfig({
  plugins: [react(), mathModule()],
  resolve: {
    alias: [
      { find: /^@cutweave\/preferences$/, replacement: `${preferencesSrc}/index.ts` },
      { find: /^@cutweave\/protocol$/, replacement: `${protocolSrc}/index.ts` },
      { find: /^@cutweave\/protocol\/(.*)$/, replacement: `${protocolSrc}/$1` },
      { find: /^@cutweave\/timeline$/, replacement: `${timelineSrc}/index.ts` },
      { find: /^@cutweave\/ui$/, replacement: `${uiSrc}/index.ts` },
      { find: /^@cutweave\/ui\/(.*)$/, replacement: `${uiSrc}/$1` },
    ],
  },
  server: { port: 5173 },
});
