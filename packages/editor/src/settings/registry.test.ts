// SPDX-License-Identifier: GPL-3.0-or-later
import { readFileSync } from "node:fs";
import { defaultPreferences, PREFERENCE_KEYS } from "@cutweave/preferences";
import { afterEach, describe, expect, it } from "vitest";
import { i18n, type TKey } from "../i18n/index.ts";
import "./core.ts";
import "./featurePages.tsx";
import {
  pageKeys,
  registeredKeys,
  registerSettingRow,
  registerSettingsPage,
  type SettingRowDef,
  type SettingsPageDef,
  settingsPages,
} from "./registry.ts";

const CATALOG = readFileSync(
  new URL("../../../../docs/reference/settings.md", import.meta.url),
  "utf8",
);

/** The first-column keys of docs/reference/settings.md: `| \`general.startup\` | … |`. */
const catalogKeys = new Set([...CATALOG.matchAll(/^\| `([a-z_.]+)`/gm)].map((m) => m[1]));

const textKeys = (row: SettingRowDef): TKey[] => [
  row.label,
  row.description,
  ...(row.keywords === undefined ? [] : [row.keywords]),
];

describe("the registry and the catalog", () => {
  it("has a row in docs/reference/settings.md for every registered key", () => {
    const keys = registeredKeys();
    expect(keys.length).toBeGreaterThan(0);
    for (const key of keys) expect(catalogKeys.has(key), key).toBe(true);
  });

  it("registers a row for every key the preferences schema defines", () => {
    expect([...registeredKeys()].sort()).toEqual([...PREFERENCE_KEYS].sort());
  });

  it("gives every row a label, a description and keywords in the English catalog", () => {
    for (const page of settingsPages()) {
      expect(i18n.exists(page.title), page.id).toBe(true);
      for (const row of page.rows) {
        for (const key of textKeys(row)) expect(i18n.exists(key), `${row.id}: ${key}`).toBe(true);
        // A row has words to be found by: written, or worked out (a shortcut's keys).
        expect(row.keywords ?? row.keywordsText, row.id).toBeDefined();
      }
    }
  });

  it("lets the chosen export folder be set only while exports go to a chosen folder", () => {
    const row = settingsPages()
      .find((p) => p.id === "export")
      ?.rows.find((r) => r.id === "export.folder_path");
    const values = defaultPreferences();
    expect(row?.disabledWhen?.(values)).toBe(true);
    expect(row?.disabledWhen?.({ ...values, "export.folder": "chosen" })).toBe(false);
  });

  it("has unique row ids, across pages too", () => {
    const ids = settingsPages().flatMap((p) => p.rows.map((r) => r.id));
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("ships its pages in the catalog's order (later work packages add theirs in between)", () => {
    const shipped = [
      "general",
      "appearance",
      "export",
      "saving",
      "shortcuts",
      "cache",
      "performance",
      "advanced",
    ];
    const ids = settingsPages().map((p) => p.id);
    expect(ids.filter((id) => shipped.includes(id))).toEqual(shipped);
  });
});

describe("registering pages and rows", () => {
  const offs: (() => void)[] = [];
  afterEach(() => {
    for (const off of offs.splice(0)) off();
  });

  const page = (extra: Partial<SettingsPageDef> = {}): SettingsPageDef => ({
    id: "demo",
    title: "settings.pages.general",
    icon: "download",
    group: "preferences",
    order: 80,
    rows: [],
    ...extra,
  });
  const row = (id: string): SettingRowDef => ({
    id,
    label: "settings.rows.theme.label",
    description: "settings.rows.theme.description",
    control: { kind: "switch" },
  });

  it("shows a page only once it has a row", () => {
    offs.push(registerSettingsPage(page()));
    expect(settingsPages().map((p) => p.id)).not.toContain("demo");
    offs.push(registerSettingRow("demo", row("demo.folder")));
    expect(settingsPages().map((p) => p.id)).toContain("demo");
  });

  it("places a page by its group and order", () => {
    const placed = () =>
      settingsPages()
        .map((p) => p.id)
        .filter((id) => ["demo", "ai", "saving", "advanced", "general"].includes(id));
    offs.push(registerSettingsPage(page({ rows: [row("demo.a")] })));
    expect(placed()).toEqual(["general", "demo", "saving", "advanced"]);
    offs.push(
      registerSettingsPage(page({ id: "ai", group: "agents", order: 1, rows: [row("ai.a")] })),
    );
    expect(placed()).toEqual(["general", "demo", "saving", "ai", "advanced"]);
  });

  it("adds a row to an existing page, and keeps one registered before its page", () => {
    offs.push(registerSettingRow("later", row("later.early")));
    offs.push(registerSettingsPage(page({ id: "later", rows: [row("later.first")] })));
    expect(
      settingsPages()
        .find((p) => p.id === "later")
        ?.rows.map((r) => r.id),
    ).toEqual(["later.first", "later.early"]);
    offs.push(registerSettingRow("saving", row("saving.extra")));
    expect(
      settingsPages()
        .find((p) => p.id === "saving")
        ?.rows.map((r) => r.id),
    ).toContain("saving.extra");
  });

  it("replaces a row with the same id and removes a page it was told to", () => {
    const off = registerSettingsPage(page({ rows: [row("demo.a")] }));
    offs.push(registerSettingRow("demo", { ...row("demo.a"), noRowReset: true }));
    const rows = settingsPages().find((p) => p.id === "demo")?.rows ?? [];
    expect(rows).toHaveLength(1);
    expect(rows[0]?.noRowReset).toBe(true);
    off();
    expect(settingsPages().map((p) => p.id)).not.toContain("demo");
  });

  it("lists the preference keys of a page's rows", () => {
    const saving = settingsPages().find((p) => p.id === "saving");
    expect(saving === undefined ? [] : pageKeys(saving)).toEqual([
      "saving.autosave",
      "saving.interval",
      "saving.backups",
    ]);
  });
});
