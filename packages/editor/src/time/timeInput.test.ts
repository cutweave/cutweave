// SPDX-License-Identifier: GPL-3.0-or-later
// `parseTimeInput` against the shared fixture file that a Rust test generates from the engine's
// own parser (`crates/cutweave-model/tests/time_input_fixture.rs`), plus a few direct cases.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { formatTimecode } from "@cutweave/timeline";
import { describe, expect, it } from "vitest";
import { parseTimeInput } from "./timeInput.ts";

interface FixtureCase {
  readonly input: string;
  readonly rate: { readonly num: number; readonly den: number } | null;
  readonly flicks: number | null;
}

const fixture = JSON.parse(
  readFileSync(
    fileURLToPath(
      new URL("../../../../crates/cutweave-model/tests/fixtures/time-input.json", import.meta.url),
    ),
    "utf8",
  ),
) as { cases: FixtureCase[] };

describe("parseTimeInput against the Rust fixture", () => {
  it("has at least 40 cases, valid and refused", () => {
    expect(fixture.cases.length).toBeGreaterThanOrEqual(40);
    expect(fixture.cases.filter((c) => c.flicks !== null).length).toBeGreaterThanOrEqual(40);
    expect(fixture.cases.filter((c) => c.flicks === null).length).toBeGreaterThanOrEqual(20);
  });

  it.each(
    fixture.cases.map(
      (c) =>
        [`${JSON.stringify(c.input)} @ ${c.rate?.num ?? "-"}/${c.rate?.den ?? "-"}`, c] as const,
    ),
  )("%s", (_name, c) => {
    const out = parseTimeInput(c.input, c.rate);
    if (c.flicks === null) {
      expect(out.ok, JSON.stringify(out)).toBe(false);
    } else {
      expect(out).toEqual({ ok: true, flicks: c.flicks });
    }
  });
});

describe("parseTimeInput", () => {
  const r30 = { num: 30, den: 1 };
  const ntsc = { num: 30_000, den: 1001 };

  it("reports why an input was refused", () => {
    expect(parseTimeInput("f:10", null)).toEqual({ ok: false, error: "needs_rate" });
    expect(parseTimeInput("00:00:00:30", r30)).toEqual({ ok: false, error: "invalid" });
    expect(parseTimeInput("hello", r30)).toEqual({ ok: false, error: "syntax" });
    expect(parseTimeInput("9007199254740992", null)).toEqual({ ok: false, error: "range" });
  });

  it("reads back what formatTimecode writes, frame exact", () => {
    for (const rate of [r30, ntsc, { num: 24, den: 1 }, { num: 60_000, den: 1001 }]) {
      for (const frame of [0, 1, 29, 30, 1799, 1800, 17_982, 107_892]) {
        const first = parseTimeInput(`f:${frame}`, rate);
        if (!first.ok) throw new Error("frame form must parse");
        const label = formatTimecode(first.flicks, rate, rate.den === 1001);
        const back = parseTimeInput(label, rate);
        expect(back, `${label} @ ${rate.num}/${rate.den}`).toEqual(first);
      }
    }
  });
});
