// SPDX-License-Identifier: GPL-3.0-or-later
// Electron fuses flipped on the packaged binary (arch §18, WP-1L.02). Fuses are compile-time
// switches baked into the Electron executable; they cannot be changed at runtime. They apply to
// packaged builds only: dev, unit tests and Playwright run the stock `node_modules/electron`
// binary, which keeps its defaults (Playwright needs the inspector flags).
// The packaging step (electron-builder `afterPack`, WP-1L.01 follow-up) calls `applyFuses`;
// `node scripts/apply-fuses.ts <app>` does the same by hand.

import { FuseV1Options, FuseVersion, flipFuses } from "@electron/fuses";

/** Desired state of every fuse in the v1 wire (strict: a new Electron fuse fails the build). */
export const FUSES = {
  /** `ELECTRON_RUN_AS_NODE` would turn the signed app into a generic Node runtime. */
  RunAsNode: false,
  /** Encrypt the cookie store at rest with an OS-protected key. */
  EnableCookieEncryption: true,
  /** `NODE_OPTIONS` could inject `--require` code into main. */
  EnableNodeOptionsEnvironmentVariable: false,
  /** `--inspect*` would open a debugger on main. */
  EnableNodeCliInspectArguments: false,
  /** Validate `app.asar` against the hash embedded at package time. */
  EnableEmbeddedAsarIntegrityValidation: true,
  /** Never load app code from a loose `app/` folder next to the asar. */
  OnlyLoadAppFromAsar: true,
  /** Not used: a single V8 snapshot is shipped. */
  LoadBrowserProcessSpecificV8Snapshot: false,
  /** The app serves no `file://` content (everything is `app://`). */
  GrantFileProtocolExtraPrivileges: false,
  /** Default (on): WebAssembly out-of-bounds traps via signal handlers. */
  WasmTrapHandlers: true,
} as const satisfies Record<keyof typeof FuseV1Options, boolean>;

export type FuseName = keyof typeof FUSES;

/** The `@electron/fuses` config for `platform` (macOS arm64 needs its ad-hoc signature reset). */
export function fuseConfig(platform: NodeJS.Platform = process.platform, arch = process.arch) {
  const wire: Partial<Record<FuseV1Options, boolean>> = {};
  for (const [name, enabled] of Object.entries(FUSES)) {
    wire[FuseV1Options[name as FuseName]] = enabled;
  }
  return {
    version: FuseVersion.V1,
    strictlyRequireAllFuses: true,
    resetAdHocDarwinSignature: platform === "darwin" && arch === "arm64",
    ...(wire as Record<FuseV1Options, boolean>),
  } as const;
}

/** Flip the fuses of the Electron binary (or `.app` bundle) at `electronPath`. */
export function applyFuses(electronPath: string): Promise<number> {
  return flipFuses(electronPath, fuseConfig());
}

/**
 * Parse `electron-fuses read --app <path>` output (`  RunAsNode is Disabled` lines; ANSI colors
 * stripped) into `{ RunAsNode: false, ... }`. Inherited/removed fuses map to null.
 */
export function parseFuseReadOutput(output: string): Record<string, boolean | null> {
  // biome-ignore lint/suspicious/noControlCharactersInRegex: strips ANSI color escapes
  const plain = output.replace(/\u001b\[[0-9;]*m/g, "");
  const fuses: Record<string, boolean | null> = {};
  for (const match of plain.matchAll(/^\s+(\w+) is (Enabled|Disabled|Inherited|Removed)\s*$/gm)) {
    const [, name, state] = match;
    if (name === undefined) continue;
    fuses[name] = state === "Enabled" ? true : state === "Disabled" ? false : null;
  }
  return fuses;
}
