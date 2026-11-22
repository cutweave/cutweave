// SPDX-License-Identifier: GPL-3.0-or-later
// `just ui-snapshots [--update]`: build Storybook, then run the Playwright suite (visual
// regression, axe, offline fonts, comparator tests). Anything after the known flags is passed
// through to `playwright test` (for example `-g "icons"`).
//
//   --update         write/refresh baselines in __screenshots__/ (per-OS baselines)
//   --skip-build     reuse an existing storybook-static/
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const cwd = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const take = (flag) => {
  const i = args.indexOf(flag);
  if (i >= 0) args.splice(i, 1);
  return i >= 0;
};
const update = take("--update");
const skipBuild = take("--skip-build");

const shell = process.platform === "win32";
const run = (cmd, cmdArgs, env = {}) => {
  const r = spawnSync(cmd, cmdArgs, {
    cwd,
    stdio: "inherit",
    shell,
    env: { ...process.env, ...env },
  });
  if (r.status !== 0) process.exit(r.status ?? 1);
};

if (!skipBuild) run("pnpm", ["run", "storybook:build"]);
run("pnpm", ["exec", "playwright", "test", ...args], update ? { UI_SNAPSHOTS_UPDATE: "1" } : {});
