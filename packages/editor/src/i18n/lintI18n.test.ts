// SPDX-License-Identifier: GPL-3.0-or-later
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, describe, expect, it } from "vitest";
import { lintSource } from "../../scripts/lint-i18n.ts";

const PKG = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const dir = mkdtempSync(join(tmpdir(), "cw-lint-i18n-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));

const rules = (source: string) => lintSource(source).map((f) => f.rule);

describe("lint:i18n", () => {
  it("rejects raw JSX text", () => {
    expect(rules("export const A = () => <p>Open project</p>;")).toEqual(["jsx-text"]);
    expect(rules("export const A = () => <p>\n  Open\n  {x}\n</p>;")).toEqual(["jsx-text"]);
  });

  it("rejects string literal children and user-facing props", () => {
    expect(rules('export const A = () => <p>{"Open"}</p>;')).toEqual(["jsx-string-child"]);
    expect(rules('export const A = () => <b aria-label="Open" />;')).toEqual(["jsx-text-prop"]);
    expect(rules('export const A = () => <b label={"Open"} />;')).toEqual(["jsx-text-prop"]);
    expect(rules('export const A = () => <b title="Open" />;')).toEqual(["jsx-text-prop"]);
  });

  it("accepts translated text, symbols and non-prose props", () => {
    const ok = `
      export const A = ({ t }) => (
        <div className="cw-x" data-tone="agent" role="group" aria-label={t("panel.assets")}>
          {t("topbar.open")}
          <span>{count}</span>
          <span>00:00 / 12</span>
          <hr />
        </div>
      );`;
    expect(lintSource(ok)).toEqual([]);
  });

  it("honours the allow marker", () => {
    expect(rules("export const A = () => <p>Open</p>; // i18n: allow")).toEqual([]);
  });

  it("reports the line of the offence", () => {
    const found = lintSource("const a = 1;\nexport const A = () => (\n  <p>Open</p>\n);");
    expect(found[0]?.line).toBe(3);
  });

  it("fails the CLI on a fixture with a raw string, and passes a clean one", () => {
    const bad = join(dir, "Bad.tsx");
    const good = join(dir, "Good.tsx");
    writeFileSync(bad, "export const Bad = () => <button>Save</button>;\n");
    writeFileSync(good, 'export const Good = ({ t }) => <button>{t("topbar.save")}</button>;\n');
    const run = (file: string) =>
      spawnSync(process.execPath, ["scripts/lint-i18n.ts", file], { cwd: PKG, encoding: "utf8" });
    const failed = run(bad);
    expect(failed.status).toBe(1);
    expect(failed.stderr).toContain("jsx-text");
    expect(run(good).status).toBe(0);
  });
});
