// SPDX-License-Identifier: GPL-3.0-or-later
// `pnpm lint:i18n`: no raw user-visible strings in JSX (every one goes through `t(...)`).
//
// Flags, in `packages/editor/src/**/*.tsx` (tests and stories excluded):
//   * JSX text containing a letter:              <p>Open</p>
//   * a string literal child in braces:          <p>{"Open"}</p>
//   * a string literal in a user-facing prop:    <IconButton label="Open" aria-label="Open" />
// A line carrying `i18n: allow` is skipped; use it only for non-prose (say why).
//
// Usage:
//   node scripts/lint-i18n.ts              # scan src/
//   node scripts/lint-i18n.ts FILE...      # scan given files (used by the fixture test)
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseAst } from "vite";

const ALLOW_MARKER = "i18n: allow";

/** Props whose string values are read by people (or assistive tech). */
export const TEXT_PROPS: ReadonlySet<string> = new Set([
  "aria-label",
  "aria-description",
  "aria-roledescription",
  "aria-valuetext",
  "alt",
  "title",
  "placeholder",
  "label",
  "description",
  "tooltip",
  "emptyText",
]);

export interface Finding {
  line: number;
  rule: "jsx-text" | "jsx-string-child" | "jsx-text-prop";
  text: string;
}

interface Node {
  type: string;
  start: number;
  end: number;
  [key: string]: unknown;
}

const hasLetter = (text: string) => /\p{L}/u.test(text);

function isNode(value: unknown): value is Node {
  return typeof value === "object" && value !== null && typeof (value as Node).type === "string";
}

function visit(node: Node, fn: (n: Node, parent: Node | null) => void, parent: Node | null = null) {
  fn(node, parent);
  for (const value of Object.values(node)) {
    if (Array.isArray(value)) {
      for (const child of value) if (isNode(child)) visit(child, fn, node);
    } else if (isNode(value)) {
      visit(value, fn, node);
    }
  }
}

/** Findings for one TSX source string. */
export function lintSource(source: string): Finding[] {
  const ast = parseAst(source, { lang: "tsx" }) as unknown as Node;
  const lineOf = (offset: number) => source.slice(0, offset).split("\n").length;
  const lines = source.split("\n");
  const findings: Finding[] = [];
  const report = (n: Node, rule: Finding["rule"], text: string) => {
    const line = lineOf(n.start);
    if (lines[line - 1]?.includes(ALLOW_MARKER)) return;
    findings.push({ line, rule, text: text.trim().slice(0, 60) });
  };

  visit(ast, (n, parent) => {
    if (n.type === "JSXText") {
      const raw = String(n.value ?? source.slice(n.start, n.end));
      if (hasLetter(raw)) report(n, "jsx-text", raw);
    } else if (
      n.type === "JSXExpressionContainer" &&
      parent &&
      /^JSX(Element|Fragment)$/.test(parent.type)
    ) {
      const expr = n.expression as Node | undefined;
      if (expr?.type === "Literal" && typeof expr.value === "string" && hasLetter(expr.value)) {
        report(n, "jsx-string-child", expr.value);
      }
    } else if (n.type === "JSXAttribute") {
      const name = n.name as Node & { name?: unknown };
      const value = n.value as Node | null;
      if (typeof name.name !== "string" || !TEXT_PROPS.has(name.name) || !value) return;
      if (value.type === "Literal" && typeof value.value === "string" && hasLetter(value.value)) {
        report(n, "jsx-text-prop", `${name.name}="${value.value}"`);
      } else if (value.type === "JSXExpressionContainer") {
        const expr = value.expression as Node;
        if (expr.type === "Literal" && typeof expr.value === "string" && hasLetter(expr.value)) {
          report(n, "jsx-text-prop", `${name.name}={"${expr.value}"}`);
        }
      }
    }
  });
  return findings;
}

function isLintable(path: string): boolean {
  return /\.tsx$/.test(path) && !/\.(test|stories)\.tsx$/.test(path);
}

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : isLintable(path) ? [path] : [];
  });
}

/** Lints files; returns the number of findings (printed as `file:line: [rule] text`). */
export function lintFiles(files: readonly string[], root: string): number {
  let count = 0;
  for (const file of files) {
    for (const f of lintSource(readFileSync(file, "utf8"))) {
      console.error(`${relative(root, file)}:${f.line}: [${f.rule}] ${f.text}`);
      count += 1;
    }
  }
  return count;
}

if (import.meta.main) {
  const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
  const args = process.argv.slice(2);
  const files = args.length > 0 ? args.map((a) => resolve(a)) : walk(join(root, "src"));
  const bad = lintFiles(files, root);
  if (bad > 0) {
    console.error(
      `\nlint:i18n: ${bad} raw string(s) in JSX. Add a key to en.json and use t("...").`,
    );
    process.exit(1);
  }
  console.log(`lint:i18n: ok (${files.length} files scanned)`);
}
