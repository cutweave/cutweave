// SPDX-License-Identifier: GPL-3.0-or-later
// `pnpm check:keymaps`: every keymap preset loads with no conflicts (WP-2H.05). Reads
// `src/keymap/keymaps/*.json`, checks each on every platform (`src/keymap/check.ts`) and exits 1
// with the problems listed. Also lists the commands marked `unverified` (notes N7) so a review
// of a preset against the program it imitates has the list.
//
// Usage:
//   node scripts/check-keymaps.ts              # check the presets in src/keymap/keymaps
//   node scripts/check-keymaps.ts FILE...      # check the given preset files
import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { CHECKED_PLATFORMS, checkPreset } from "../src/keymap/check.ts";
import type { PresetFile } from "../src/keymap/preset.ts";

const here = fileURLToPath(new URL(".", import.meta.url));
const dir = resolve(here, "../src/keymap/keymaps");
const files =
  process.argv.length > 2
    ? process.argv.slice(2)
    : readdirSync(dir)
        .filter((f) => f.endsWith(".json"))
        .map((f) => join(dir, f));

let failed = false;
for (const file of files) {
  const preset = JSON.parse(readFileSync(file, "utf8")) as PresetFile;
  const problems = CHECKED_PLATFORMS.flatMap((platform) => checkPreset(preset, platform));
  const unverified = preset.bindings.filter((b) => b.unverified === true).map((b) => b.command);
  console.log(
    `${preset.id}: ${preset.bindings.length} bindings, ${problems.length} problems` +
      (unverified.length > 0 ? `, unverified: ${[...new Set(unverified)].join(", ")}` : ""),
  );
  for (const problem of problems) {
    failed = true;
    console.error(`  ${problem.preset} (${problem.platform}): ${problem.message}`);
  }
}
process.exit(failed ? 1 : 0);
