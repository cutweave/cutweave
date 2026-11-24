// SPDX-License-Identifier: GPL-3.0-or-later
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { isPreferenceKey } from "@cutweave/preferences";
import { describe, expect, it } from "vitest";
import { createI18n } from "./index.ts";
import en from "./locales/en.json";

const SRC = join(dirname(fileURLToPath(import.meta.url)), "..");

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sources(path);
    return /\.(ts|tsx)$/.test(name) && !/\.(test|stories)\./.test(name) ? [path] : [];
  });
}

function flatten(node: unknown, prefix = ""): string[] {
  if (typeof node === "string") return [prefix];
  if (typeof node === "object" && node !== null) {
    return Object.entries(node).flatMap(([k, v]) => flatten(v, prefix ? `${prefix}.${k}` : k));
  }
  return [];
}

const catalogKeys = flatten(en);
const code = sources(SRC)
  .map((file) => readFileSync(file, "utf8"))
  .join("\n");
/** Every string literal in the code that looks like a catalog key (dotted, camel/kebab segments). */
const KEY_LIKE = /"([a-z][A-Za-z0-9-]*(?:\.[A-Za-z0-9-]+)+)"/g;
const referenced = new Set([...code.matchAll(KEY_LIKE)].map((m) => m[1] as string));

describe("i18n catalog", () => {
  it("has a non-empty string for every key", () => {
    expect(catalogKeys.length).toBeGreaterThan(50);
    const values = (node: unknown): string[] =>
      typeof node === "string" ? [node] : Object.values(node as object).flatMap(values);
    for (const value of values(en)) expect(value.trim()).not.toBe("");
  });

  it("covers every key the code references", () => {
    const missing = [...referenced].filter((key) => {
      // A preference key (`export.hw`) is not a translation key, even where it shares a root.
      if (isPreferenceKey(key)) return false;
      // Only dotted strings under a catalog root count as translation keys.
      const root = key.split(".")[0] as string;
      return root in en && !catalogKeys.includes(key);
    });
    expect(missing).toEqual([]);
  });

  it("is fully used: every catalog key is referenced by the code", () => {
    const unused = catalogKeys.filter((key) => !referenced.has(key));
    expect(unused).toEqual([]);
  });

  it("resolves keys and interpolates values through i18next", () => {
    const i18n = createI18n("en");
    expect(i18n.t("topbar.export")).toBe("Export");
    expect(i18n.t("timeline.trackVideo", { n: 2 })).toBe("Video 2");
    expect(i18n.t("workspace.short-form")).toBe("Short-form");
  });

  it("names every asset browser tab from the design brief", () => {
    const tabs = Object.values(en.assets.tabs);
    expect(tabs).toEqual([
      "Media",
      "Audio",
      "Text",
      "Stickers",
      "Effects",
      "Transitions",
      "Filters",
      "Templates",
      "Captions",
      "AI Studio",
    ]);
  });
});
