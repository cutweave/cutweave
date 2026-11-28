// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "vitest/config";

// `pnpm --filter @cutweave/desktop test`: unit tests only; Playwright specs live in e2e/.
export default defineConfig({ test: { include: ["src/**/*.test.ts", "e2e/perf/**/*.test.ts"] } });
