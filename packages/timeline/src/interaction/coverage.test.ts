// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.08 coverage check: the state chart in packages/timeline/README.md is the machine. Every
// state the chart names exists, every edge the transcripts take is in the chart, and every edge
// in the chart is taken by at least one transcript (100% of states and transitions).
// Run alone: `pnpm vitest run packages/timeline/src/interaction/coverage.test.ts`.
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath } from "@cutweave/protocol/wasm";
import { beforeAll, describe, expect, it } from "vitest";
import { MACHINE_STATES } from "./state.ts";
import { edgeKey, parseChart, readChart } from "./testing/chart.ts";
import { replay } from "./testing/transcript.ts";
import { TRANSCRIPTS } from "./testing/transcripts.ts";
import { World } from "./testing/world.ts";

beforeAll(async () => {
  await initMath(mathModuleBytes());
}, 900_000);

describe("the documented state chart", () => {
  const chart = readChart();

  it("names exactly the machine's states", () => {
    expect([...chart.states].sort()).toEqual([...MACHINE_STATES].sort());
  });

  it("is covered by the transcripts, edge for edge, and the machine takes no other edge", () => {
    const taken = new Map<string, number>();
    const visited = new Set<string>(["idle"]);
    for (const t of TRANSCRIPTS) {
      const r = replay(t, (o) => new World(o));
      for (const e of r.world.edges) {
        const k = edgeKey(e);
        taken.set(k, (taken.get(k) ?? 0) + 1);
        visited.add(e.to);
      }
    }
    const undocumented = [...taken.keys()].filter((k) => !chart.edges.has(k)).sort();
    const uncovered = [...chart.edges].filter((k) => !taken.has(k)).sort();
    const unvisited = MACHINE_STATES.filter((s) => !visited.has(s));
    const covered = chart.edges.size - uncovered.length;
    // The report the coverage script prints.
    console.info(
      `state chart: ${visited.size}/${MACHINE_STATES.length} states, ` +
        `${covered}/${chart.edges.size} transitions covered by ${TRANSCRIPTS.length} transcripts`,
    );
    expect(undocumented).toEqual([]);
    expect(uncovered).toEqual([]);
    expect(unvisited).toEqual([]);
  });

  it("parses edges with labels and rejects a chart without the diagram", () => {
    const md = ["```mermaid", "stateDiagram-v2", "    [*] --> a", "    a --> b : go [x]", "```"];
    const c = parseChart(md.join("\n"));
    expect([...c.states]).toEqual(["a", "b"]);
    expect([...c.edges]).toEqual(["a --> b : go [x]"]);
    expect(() => parseChart("no chart")).toThrow(/stateDiagram/);
  });
});
