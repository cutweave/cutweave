// SPDX-License-Identifier: GPL-3.0-or-later
// Flip Cutweave's Electron fuses (src/build/fuses.ts) on a packaged app:
//   node scripts/apply-fuses.ts <path to electron binary or .app bundle>
// The packaging step calls `applyFuses` from electron-builder's `afterPack`; this is the manual
// equivalent. Verify with `npx @electron/fuses read --app <path>`.

import { applyFuses } from "../src/build/fuses.ts";

const target = process.argv[2];
if (target === undefined) {
  console.error("usage: node scripts/apply-fuses.ts <electron binary or .app>");
  process.exit(2);
}
await applyFuses(target);
console.log(`apply-fuses: fuses flipped on ${target}`);
