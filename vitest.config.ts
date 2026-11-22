// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "vitest/config";

// One root run covers every workspace package.
export default defineConfig({
  test: {
    include: ["{packages,apps}/*/src/**/*.test.ts"],
    passWithNoTests: false,
  },
});
