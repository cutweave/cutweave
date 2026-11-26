// SPDX-License-Identifier: GPL-3.0-or-later
// Startup of the shared math module (arch §10, ADR-0033): the editor loads it before it renders
// anything that converts a time, draws a curve or snaps. A module that does not match this build
// of `@cutweave/protocol` (or cannot load at all) stops the editor with a clear message instead of
// letting it run with wrong numbers.

import { initMath, MathVersionError } from "@cutweave/protocol/wasm";

export type MathStartup =
  | { readonly ok: true }
  | {
      readonly ok: false;
      /** `version`: the module does not match this build; `load`: it could not be loaded at all. */
      readonly kind: "version" | "load";
      readonly detail: string;
    };

/** Loads the math module from `bytes`; never rejects. */
export async function startMath(
  bytes: () => Promise<{ default: Uint8Array }>,
): Promise<MathStartup> {
  try {
    await initMath((await bytes()).default);
    return { ok: true };
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    return { ok: false, kind: error instanceof MathVersionError ? "version" : "load", detail };
  }
}
