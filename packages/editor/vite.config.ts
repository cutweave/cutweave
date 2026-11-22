// SPDX-License-Identifier: GPL-3.0-or-later
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const uiSrc = fileURLToPath(new URL("../ui/src", import.meta.url));

// Plain Vite app: `pnpm --filter @cutweave/editor dev` runs the editor with no engine.
// `@cutweave/ui` resolves to its source so the editor needs no prior `ui` build.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [
      { find: /^@cutweave\/ui$/, replacement: `${uiSrc}/index.ts` },
      { find: /^@cutweave\/ui\/(.*)$/, replacement: `${uiSrc}/$1` },
    ],
  },
  server: { port: 5173 },
});
