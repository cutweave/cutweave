// SPDX-License-Identifier: GPL-3.0-or-later
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const uiSrc = fileURLToPath(new URL("./packages/ui/src", import.meta.url));

// One root run covers every workspace package.
export default defineConfig({
  // Workspace packages import `@cutweave/ui` as source (its `main` points at an unbuilt dist/).
  resolve: {
    alias: [
      { find: /^@cutweave\/ui$/, replacement: `${uiSrc}/index.ts` },
      { find: /^@cutweave\/ui\/(.*)$/, replacement: `${uiSrc}/$1` },
    ],
  },
  test: {
    include: ["{packages,apps}/*/src/**/*.test.ts"],
    passWithNoTests: false,
  },
});
