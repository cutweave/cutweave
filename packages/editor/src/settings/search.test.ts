// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { i18n, type TKey } from "../i18n/index.ts";
import "./core.ts";
import { settingsPages } from "./registry.ts";
import { countMatches, fold, markMatches, queryWords, searchSettings } from "./search.ts";

const t = (key: TKey): string => i18n.t(key) as unknown as string;
const find = (query: string) => searchSettings(settingsPages(), query, t);

describe("settings search", () => {
  it("folds case and accents", () => {
    expect(fold("Café ÉCLAIR")).toBe("cafe eclair");
    expect(queryWords("  Save   AUTO ")).toEqual(["save", "auto"]);
    expect(queryWords("   ")).toEqual([]);
  });

  it("finds nothing for a blank query", () => {
    expect(find("")).toEqual([]);
    expect(find("   ")).toEqual([]);
  });

  it("matches labels, descriptions and keywords, in any word order", () => {
    // label
    expect(find("theme")[0]?.rows[0]?.row.id).toBe("appearance.theme");
    // description
    expect(find("follow your system")[0]?.rows[0]?.row.id).toBe("appearance.theme");
    // keywords only ("night" is not in the label or the description)
    expect(find("night")[0]?.rows[0]?.row.id).toBe("appearance.theme");
    // any order, any case
    expect(find("PROJECTS recent")[0]?.rows.map((r) => r.row.id)).toContain("general.recent_count");
  });

  it("needs every word to match", () => {
    expect(find("theme zzzz")).toEqual([]);
  });

  it("groups the rows by page, in page order, and counts them", () => {
    const found = find("auto");
    expect(found.map((m) => m.page.id)).toEqual(["saving"]);
    expect(found[0]?.rows.map((r) => r.row.id)).toEqual([
      "saving.autosave",
      "saving.interval",
      "saving.backups",
    ]);
    expect(countMatches(found)).toBe(3);
  });

  it("matches on the page title, so a page name finds its rows", () => {
    expect(find("appearance")[0]?.rows.map((r) => r.row.id)).toEqual(["appearance.theme"]);
  });

  it("marks the matching text, in the label and the description", () => {
    const row = find("auto")[0]?.rows[0];
    expect(row?.label).toEqual([
      { text: "Save ", match: false },
      { text: "auto", match: true },
      { text: "matically", match: false },
    ]);
    expect(markMatches("Keep the Café open", ["cafe"])).toEqual([
      { text: "Keep the ", match: false },
      { text: "Café", match: true },
      { text: " open", match: false },
    ]);
    expect(markMatches("abab", ["ab"])).toEqual([{ text: "abab", match: true }]);
    expect(markMatches("nothing", ["zz"])).toEqual([{ text: "nothing", match: false }]);
  });
});
