// SPDX-License-Identifier: GPL-3.0-or-later
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { isPreferenceKey } from "@cutweave/preferences";
import { describe, expect, it } from "vitest";
import { COMMAND_INFO } from "../menus/builtins.ts";
import { ZONE_SURFACE } from "../menus/canvas.ts";
import { hintBindingIds } from "../menus/keys.ts";
import {
  HINT_BINDING_IDS,
  SURFACE_COMMAND_INFO,
  SURFACE_IDS,
  UI_COMMAND_IDS,
} from "../menus/surfaces/index.ts";
import { CONTEXT_KEYS } from "../menus/when.ts";
import { createI18n, detectLanguage } from "./index.ts";
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
// Names of registry commands, surfaces, context keys and bindings share roots with the catalog
// (`timeline.clip`, `menu.undo`, `engine.live`); the menu framework's tables own them (WP-2H.12).
const MENU_IDS: ReadonlySet<string> = new Set([
  ...CONTEXT_KEYS,
  ...COMMAND_INFO.map((c) => c.name),
  ...hintBindingIds(),
  ...HINT_BINDING_IDS,
  ...SURFACE_IDS,
  ...UI_COMMAND_IDS,
  ...SURFACE_COMMAND_INFO.map((c) => c.name),
  ...Object.values(ZONE_SURFACE).filter((id): id is string => id !== null),
  "timeline.canvas",
  // The command palette's row surface and its row command, and its shortcut's binding id (WP-2H.04).
  "palette.row",
  "palette.run",
  "menu.commandPalette",
]);
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
      if (MENU_IDS.has(key)) return false;
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

  it("picks the first preferred language that has a catalog, else English", () => {
    expect(detectLanguage(["en-GB", "he"])).toBe("en");
    expect(detectLanguage(["he", "fr-CA", "en-US"])).toBe("en");
    expect(detectLanguage(["he", "fr"])).toBe("en");
    expect(detectLanguage([])).toBe("en");
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
