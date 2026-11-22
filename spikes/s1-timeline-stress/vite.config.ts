// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "vite";

// Cross-origin isolation lets performance.measureUserAgentSpecificMemory() work.
const isolation = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

export default defineConfig({
  server: { headers: isolation },
  preview: { headers: isolation },
  build: { target: "es2023" },
});
