// SPDX-License-Identifier: GPL-3.0-or-later
// pnpm does not run dependency postinstall scripts here (see pnpm-workspace.yaml), so fetch the
// Electron binary on demand before e2e. A blocked download fails loudly; nothing is worked around.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";

const require = createRequire(import.meta.url);
const electronDir = path.dirname(require.resolve("electron/package.json"));

if (!existsSync(path.join(electronDir, "path.txt"))) {
  console.log("ensure-electron: downloading the Electron binary");
  const result = spawnSync(process.execPath, [path.join(electronDir, "install.js")], {
    stdio: "inherit",
  });
  if (result.status !== 0) {
    console.error("ensure-electron: Electron download failed");
    process.exit(result.status ?? 1);
  }
}
