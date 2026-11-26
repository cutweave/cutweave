// SPDX-License-Identifier: GPL-3.0-or-later
// Test support (not built): reads the state chart documented in `packages/timeline/README.md`
// (the mermaid `stateDiagram-v2` block), so the tests can hold the machine to it.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { ChartEdge } from "../reducer.ts";

export const README = fileURLToPath(new URL("../../../README.md", import.meta.url));

export interface Chart {
  /** The states the chart names (without `[*]`). */
  readonly states: ReadonlySet<string>;
  /** `from --> to : label` for every edge. */
  readonly edges: ReadonlySet<string>;
}

export const edgeKey = (e: ChartEdge): string => `${e.from} --> ${e.to} : ${e.label}`;

const EDGE = /^\s*(\[\*\]|\w+)\s*-->\s*(\w+)\s*(?::\s*(.+?))?\s*$/;

export function parseChart(markdown: string): Chart {
  const lines = markdown.split("\n");
  const start = lines.findIndex((l) => l.trim() === "```mermaid");
  if (start < 0 || lines[start + 1]?.trim() !== "stateDiagram-v2") {
    throw new Error("no mermaid stateDiagram-v2 block");
  }
  const states = new Set<string>();
  const edges = new Set<string>();
  for (let i = start + 2; i < lines.length && lines[i]?.trim() !== "```"; i++) {
    const m = EDGE.exec(lines[i] ?? "");
    if (m === null) continue;
    const [, from, to, label] = m;
    if (from === undefined || to === undefined) continue;
    if (from !== "[*]") states.add(from);
    states.add(to);
    if (from === "[*]") continue;
    if (label === undefined) throw new Error(`edge without a label: ${lines[i]}`);
    edges.add(`${from} --> ${to} : ${label}`);
  }
  return { states, edges };
}

export function readChart(): Chart {
  return parseChart(readFileSync(README, "utf8"));
}
