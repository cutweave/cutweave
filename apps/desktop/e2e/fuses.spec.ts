// SPDX-License-Identifier: GPL-3.0-or-later
// Fuses (WP-1L.02): flip Cutweave's fuse config on a copy of the Electron binary, the same call the
// packaging step makes, then read it back with the `@electron/fuses` CLI and parse the output.
// Packaging itself (electron-builder `afterPack`) is wired by the packaging WP.

import { execFileSync } from "node:child_process";
import { copyFileSync, cpSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { applyFuses, FUSES, parseFuseReadOutput } from "../src/build/fuses.ts";

const require = createRequire(import.meta.url);

test("the packaged binary's fuses read back as configured", async () => {
  test.setTimeout(120_000);
  const electronDir = path.dirname(require.resolve("electron/package.json"));
  const relative = readFileSync(path.join(electronDir, "path.txt"), "utf8").trim();
  const tmp = mkdtempSync(path.join(tmpdir(), "cutweave-fuses-"));
  try {
    let target: string;
    if (process.platform === "darwin") {
      target = path.join(tmp, "Electron.app");
      cpSync(path.join(electronDir, "dist", "Electron.app"), target, {
        recursive: true,
        verbatimSymlinks: true,
      });
    } else {
      target = path.join(tmp, path.basename(relative));
      copyFileSync(path.join(electronDir, "dist", relative), target);
    }

    const stock = parseFuseReadOutput(readFuses(target));
    expect(stock.RunAsNode).toBe(true);

    await applyFuses(target);
    expect(parseFuseReadOutput(readFuses(target))).toEqual(FUSES);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});

/** `npx @electron/fuses read --app <target>`, run through the pinned package's CLI. */
function readFuses(target: string): string {
  const cli = path.join(path.dirname(require.resolve("@electron/fuses")), "bin.js");
  // Colors are stripped by the parser.
  return execFileSync(process.execPath, [cli, "read", "--app", target], { encoding: "utf8" });
}
