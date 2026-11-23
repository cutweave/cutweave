// SPDX-License-Identifier: GPL-3.0-or-later
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const uiSrc = fileURLToPath(new URL("../ui/src", import.meta.url));
const protocolSrc = fileURLToPath(new URL("../protocol/src", import.meta.url));
const timelineSrc = fileURLToPath(new URL("../timeline/src", import.meta.url));

// Plain Vite app: `pnpm --filter @cutweave/editor dev` runs the editor with no engine.
// `@cutweave/ui`, `/protocol` and `/timeline` resolve to their source so the editor needs no prior build.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [
      { find: /^@cutweave\/protocol$/, replacement: `${protocolSrc}/index.ts` },
      { find: /^@cutweave\/protocol\/(.*)$/, replacement: `${protocolSrc}/$1` },
      { find: /^@cutweave\/timeline$/, replacement: `${timelineSrc}/index.ts` },
      { find: /^@cutweave\/ui$/, replacement: `${uiSrc}/index.ts` },
      { find: /^@cutweave\/ui\/(.*)$/, replacement: `${uiSrc}/$1` },
    ],
  },
  server: { port: 5173 },
});
