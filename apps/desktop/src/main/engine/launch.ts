// SPDX-License-Identifier: GPL-3.0-or-later
// Where the engine binary lives and how to start it (launch protocol, WP-1I.04).

import { existsSync } from "node:fs";
import path from "node:path";

export interface EngineLaunch {
  command: string;
  /** Arguments before the per-launch ones (`--port N` etc. are appended by the supervisor). */
  args: string[];
}

export interface ResolveOptions {
  env: Readonly<Record<string, string | undefined>>;
  isPackaged: boolean;
  /** `process.resourcesPath` of the packaged app (engine at `resources/engine/`). */
  resourcesPath: string;
  /** Repository root, for the dev builds under `target/debug/`. */
  repoRoot: string;
  platform: NodeJS.Platform;
  exists?: (file: string) => boolean;
}

/**
 * Pick the engine to supervise, or null when none is available (the UI then runs engine-less).
 * Order: dev/test fake via `CUTWEAVE_ENGINE_FAKE=<script>`; explicit `CUTWEAVE_ENGINE_BIN`
 * (both never when packaged); packaged `resources/engine/cutweave`; dev `target/debug/cutweave`,
 * then `target/debug/cutweave-engine-serve` (the launch-protocol binary until `cutweave engine`
 * ships with WP-1K.01). Every candidate takes the `engine` subcommand.
 */
export function resolveEngineLaunch(options: ResolveOptions): EngineLaunch | null {
  const { env, isPackaged, platform } = options;
  const exists = options.exists ?? existsSync;
  const exe = (name: string) => (platform === "win32" ? `${name}.exe` : name);

  if (!isPackaged) {
    const fake = env.CUTWEAVE_ENGINE_FAKE;
    if (fake) return { command: "node", args: [fake, "engine"] };
    const override = env.CUTWEAVE_ENGINE_BIN;
    if (override) return { command: override, args: ["engine"] };
  }

  const candidates = isPackaged
    ? [path.join(options.resourcesPath, "engine", exe("cutweave"))]
    : [
        path.join(options.repoRoot, "target", "debug", exe("cutweave")),
        path.join(options.repoRoot, "target", "debug", exe("cutweave-engine-serve")),
      ];
  const found = candidates.find((c) => exists(c));
  return found === undefined ? null : { command: found, args: ["engine"] };
}
