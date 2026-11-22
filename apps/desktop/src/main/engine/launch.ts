// SPDX-License-Identifier: GPL-3.0-or-later
// Where the engine binary lives and how to start it (launch protocol, WP-1I.04).

import { existsSync } from "node:fs";
import net from "node:net";
import path from "node:path";

export interface EngineLaunch {
  command: string;
  /** Arguments before the per-launch ones (`--port N` is appended by the supervisor). */
  args: string[];
}

export interface ResolveOptions {
  env: Readonly<Record<string, string | undefined>>;
  isPackaged: boolean;
  /** `process.resourcesPath` of the packaged app (engine at `resources/engine/`). */
  resourcesPath: string;
  /** Repository root, for the dev build at `target/debug/cutweave`. */
  repoRoot: string;
  platform: NodeJS.Platform;
  exists?: (file: string) => boolean;
}

/**
 * Pick the engine to supervise, or null when none is available (the UI then runs engine-less).
 * Order: explicit `CUTWEAVE_ENGINE_BIN`; packaged `resources/engine/cutweave`; dev
 * `target/debug/cutweave`; dev/test fake via `CUTWEAVE_ENGINE_FAKE=<script>` (never when packaged).
 */
export function resolveEngineLaunch(options: ResolveOptions): EngineLaunch | null {
  const { env, isPackaged, platform } = options;
  const exists = options.exists ?? existsSync;
  const exe = platform === "win32" ? "cutweave.exe" : "cutweave";

  if (!isPackaged) {
    const fake = env.CUTWEAVE_ENGINE_FAKE;
    if (fake) return { command: "node", args: [fake, "engine"] };
  }
  const override = env.CUTWEAVE_ENGINE_BIN;
  if (override && !isPackaged) return { command: override, args: ["engine"] };

  const candidate = isPackaged
    ? path.join(options.resourcesPath, "engine", exe)
    : path.join(options.repoRoot, "target", "debug", exe);
  return exists(candidate) ? { command: candidate, args: ["engine"] } : null;
}

/** Ask the OS for a free loopback port. Main chooses it once at launch (arch §6). */
export function pickFreePort(host = "127.0.0.1"): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.once("error", reject);
    server.listen(0, host, () => {
      const address = server.address();
      server.close(() => {
        if (address !== null && typeof address === "object") resolve(address.port);
        else reject(new Error("no port assigned"));
      });
    });
  });
}
