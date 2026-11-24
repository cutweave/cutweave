// SPDX-License-Identifier: GPL-3.0-or-later
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const uiSrc = fileURLToPath(new URL("./packages/ui/src", import.meta.url));
const preferencesSrc = fileURLToPath(new URL("./packages/preferences/src", import.meta.url));
const protocolSrc = fileURLToPath(new URL("./packages/protocol/src", import.meta.url));
const timelineSrc = fileURLToPath(new URL("./packages/timeline/src", import.meta.url));

// One root run covers every workspace package.
export default defineConfig({
  // Workspace packages import `@cutweave/ui`, `/protocol` and `/timeline` as source (their `main`
  // points at an unbuilt dist/).
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
  test: {
    include: ["{packages,apps}/*/src/**/*.test.ts"],
    passWithNoTests: false,
    // The merge gate often runs while agents compile Rust in parallel: cap workers and give
    // worker startup / slow jsdom suites headroom so load never turns into a false failure.
    maxWorkers: 4,
    testTimeout: 20_000,
    hookTimeout: 30_000,
    teardownTimeout: 30_000,
  },
});
