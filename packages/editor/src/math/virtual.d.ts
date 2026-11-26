// SPDX-License-Identifier: GPL-3.0-or-later
// The bundled math module's bytes (vite.config.ts, ADR-0033).
declare module "virtual:cutweave-math-wasm" {
  const bytes: Uint8Array;
  export default bytes;
}
