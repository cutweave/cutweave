// SPDX-License-Identifier: GPL-3.0-or-later
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

export const PKG_ROOT = resolve(here, "..");
export const STORYBOOK_DIR = resolve(PKG_ROOT, "storybook-static");

/**
 * Baseline class (docs/01-architecture.md §20: baselines are per adapter). Screenshots are taken
 * by Chromium with software rendering (no GPU) on Linux; other adapters get their own directory.
 */
// One baseline set per OS (fonts and anti-aliasing differ); each OS compares only against its own.
const OS_NAME =
  process.platform === "win32" ? "windows" : process.platform === "darwin" ? "macos" : "linux";
export const ADAPTER_CLASS = `${OS_NAME}-chromium-sw`;
export const BASELINE_DIR = resolve(PKG_ROOT, "__screenshots__", ADAPTER_CLASS);
export const ACTUAL_DIR = resolve(PKG_ROOT, "__screenshots-actual__", ADAPTER_CLASS);

export const PORT = 6007;
export const BASE_URL = `http://127.0.0.1:${PORT}`;
export const VIEWPORT = { width: 1280, height: 800 } as const;

export const THEMES = ["dark", "light"] as const;
export type Theme = (typeof THEMES)[number];

/** Regression tolerances, single source: docs/01-architecture.md §20. */
export const SSIM_MIN = 0.995;
export const PSNR_MIN = 40;

export interface StoryEntry {
  id: string;
  title: string;
  name: string;
}

/** Every story in the static build, read from storybook-static/index.json. */
export function listStories(): StoryEntry[] {
  const file = resolve(STORYBOOK_DIR, "index.json");
  if (!existsSync(file)) {
    throw new Error(
      `${file} not found: run \`pnpm storybook:build\` first (or use \`just ui-snapshots\`).`,
    );
  }
  const index = JSON.parse(readFileSync(file, "utf8")) as {
    entries: Record<string, { id: string; type: string; title: string; name: string }>;
  };
  return Object.values(index.entries)
    .filter((e) => e.type === "story")
    .map(({ id, title, name }) => ({ id, title, name }))
    .sort((a, b) => a.id.localeCompare(b.id));
}

/** True when no baseline PNG has been committed yet (first CI run on a fresh checkout). */
export function hasAnyBaseline(): boolean {
  const walk = (dir: string): boolean =>
    existsSync(dir) &&
    readdirSync(dir, { withFileTypes: true }).some((d) =>
      d.isDirectory() ? walk(resolve(dir, d.name)) : d.name.endsWith(".png"),
    );
  return walk(BASELINE_DIR);
}
