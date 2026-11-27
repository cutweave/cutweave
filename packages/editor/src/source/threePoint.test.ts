// SPDX-License-Identifier: GPL-3.0-or-later
// The `three_point` fixtures of the timeline spec (docs/spec/fixtures, §4.18) are the truth: the
// Source view's placement math must give exactly what they expect (WP-2A.14).
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { threePoint } from "./threePoint.ts";

interface Fixture {
  readonly kind: string;
  readonly id: string;
  readonly title: string;
  readonly fps: number;
  readonly media: { readonly kind: string; readonly frames?: number };
  readonly playhead: number;
  readonly timeline_in?: number;
  readonly timeline_out?: number;
  readonly source_in?: number;
  readonly source_out?: number;
  readonly expected?: {
    readonly at: number;
    readonly source_in: number;
    readonly duration: number;
    readonly shortened: boolean;
  };
  readonly error?: { readonly code: string };
}

const DIR = join(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "..",
  "..",
  "docs",
  "spec",
  "fixtures",
);

const fixtures: Fixture[] = readdirSync(DIR)
  .filter((name) => name.endsWith(".json"))
  .map((name) => JSON.parse(readFileSync(join(DIR, name), "utf8")) as Fixture)
  .filter((fx) => fx.kind === "three_point");

/** The fixture's numbers are frames; the function works in any one unit. */
const run = (fx: Fixture) =>
  threePoint({
    playhead: fx.playhead,
    timeline:
      fx.timeline_in === undefined || fx.timeline_out === undefined
        ? null
        : { in: fx.timeline_in, out: fx.timeline_out },
    sourceIn: fx.source_in ?? null,
    sourceOut: fx.source_out ?? null,
    mediaLength: fx.media.kind === "image" ? null : (fx.media.frames ?? null),
    stillLength: 5 * fx.fps,
  });

describe("three-point placement (spec §4.18)", () => {
  it("has the fixtures of every row of the table", () => {
    const ids = fixtures.map((fx) => fx.id);
    expect(ids).toEqual(
      expect.arrayContaining([
        "three-point-no-marks",
        "three-point-source-marks",
        "three-point-timeline-marks",
        "three-point-both-marks",
      ]),
    );
  });

  it.each(fixtures.map((fx) => [fx.id, fx] as const))("%s", (_id, fx) => {
    const got = run(fx);
    if (fx.error !== undefined) {
      expect(got).toEqual({ ok: false });
      return;
    }
    expect(got).toEqual({
      ok: true,
      at: fx.expected?.at,
      sourceIn: fx.expected?.source_in,
      duration: fx.expected?.duration,
      shortened: fx.expected?.shortened,
    });
  });

  it("refuses a timeline range without a length and a source in at the media end", () => {
    const base = {
      playhead: 0,
      timeline: null,
      sourceIn: null,
      sourceOut: null,
      mediaLength: 100,
      stillLength: 125,
    };
    expect(threePoint({ ...base, timeline: { in: 10, out: 10 } })).toEqual({ ok: false });
    expect(threePoint({ ...base, sourceIn: 100 })).toEqual({ ok: false });
  });
});
