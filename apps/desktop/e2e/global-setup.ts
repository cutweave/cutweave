// SPDX-License-Identifier: GPL-3.0-or-later
// Runs once before the e2e suite: the real engine binary must load. One the OS cannot start (a
// missing FFmpeg DLL on Windows, 0xC0000135; a missing shared library elsewhere, exit 127) would
// otherwise fail every real-engine test, each through the app's "engine stopped" dialog. Fail
// the run here instead, once, with the reason.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { realEngine } from "./fixtures.ts";

/** Exit codes from the OS loader, not from the engine (which exits with 0-13). */
const LOADER_EXITS: Readonly<Record<number, string>> = {
  [0xc0000135]: "a DLL it needs is missing (STATUS_DLL_NOT_FOUND)",
  [0xc0000139]: "a DLL it loaded is the wrong version (STATUS_ENTRYPOINT_NOT_FOUND)",
  127: "a shared library it needs is missing",
};

export default function globalSetup(): void {
  // Without the binary, the real-engine tests skip themselves.
  if (!existsSync(realEngine)) return;
  // `--help` is a usage error: the engine prints one JSON line and exits 2 without starting.
  const run = spawnSync(realEngine, ["--help"], { timeout: 20_000, windowsHide: true });
  const status = run.status;
  const why =
    run.error !== undefined
      ? run.error.message
      : status === null
        ? `it did not exit (signal ${run.signal ?? "none"})`
        : LOADER_EXITS[status];
  if (why === undefined) return;
  throw new Error(
    [
      `The engine binary cannot start: ${why}.`,
      `  binary: ${realEngine}`,
      status === null ? "" : `  exit code: ${status} (0x${status.toString(16).toUpperCase()})`,
      `  ${run.stderr?.toString().trim() ?? ""}`,
      "Rebuild it (cargo build -p cutweave-engine --bin cutweave-engine-serve). A build with the",
      "ffmpeg feature places the FFmpeg DLLs next to it on Windows (docs/dev/ffmpeg.md).",
    ]
      .filter((l) => l.trim() !== "")
      .join("\n"),
  );
}
