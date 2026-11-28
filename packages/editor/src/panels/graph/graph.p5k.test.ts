// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.07 against the P-5K benchmark project (WP-2J.01: 5,000 items, 50,000 keyframes): the
// graph editor on its most heavily keyed clip, and on the eight most heavily keyed clips at
// once. The curves are built from the real document, sampled by the engine's math (the wasm
// module) and drawn through pans and zooms; a document patch elsewhere costs nothing. Skips when
// the project has not been generated (`cargo xtask gen-bench-project --preset p-5k`;
// `testdata/bench-projects/` is not committed). The canvas is not rasterized here: the time is
// the editor's own work per frame (sampling, paths), which a real canvas adds its fill to.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { ClipItem, Project, SequenceId } from "@cutweave/protocol";
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath, math } from "@cutweave/protocol/wasm";
import { type GraphTheme, GraphView } from "@cutweave/ui";
import { beforeAll, describe, expect, it } from "vitest";
import { ANIMATABLE_PROPS, storedKeys } from "../../timeline/keyframes.ts";
import { buildCurves, CompiledCache, sameClips, selectedClips } from "./graphModel.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..", "..");
const FOLDER = join(ROOT, "testdata", "bench-projects", "P-5K");
const DOCUMENT = existsSync(FOLDER)
  ? readdirSync(FOLDER).find((name) => name.endsWith(".cutweave") && name !== ".cutweave")
  : undefined;

const THEME: GraphTheme = {
  surface: "",
  ruler: "",
  grid: "",
  gridStrong: "",
  text: "",
  textStrong: "",
  playhead: "",
  selected: "",
  onSelected: "",
  marquee: "",
  guide: "",
  curves: ["a", "b", "c", "d", "e", "f", "g", "h"],
  font: "",
  fontPx: 12,
  dimmed: 0.5,
  line: 1,
  lineStrong: 2,
  rulerHeight: 24,
  rulerWidth: 64,
  keySize: 8,
  handleSize: 6,
  hitRadius: 8,
  pad: 4,
};

const noopContext = new Proxy(
  {},
  {
    get: (_t, prop) => (prop === "measureText" ? () => ({ width: 0 }) : () => undefined),
    set: () => true,
  },
) as CanvasRenderingContext2D;

const F = 705_600_000;

const keyCount = (clip: ClipItem): number =>
  ANIMATABLE_PROPS.reduce((n, prop) => n + (storedKeys(clip, prop)?.length ?? 0), 0);

const p95 = (values: number[]): number => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.ceil(0.95 * sorted.length) - 1] ?? 0;
};

beforeAll(async () => {
  await initMath(mathModuleBytes());
}, 900_000);

describe("the graph editor on the P-5K benchmark project", () => {
  it.skipIf(DOCUMENT === undefined)(
    "builds, samples and draws the most heavily keyed clips well inside a frame",
    () => {
      const project = JSON.parse(readFileSync(join(FOLDER, DOCUMENT as string), "utf8")) as Project;
      const sequence = Object.keys(project.sequences)[0] as SequenceId;
      const clips = Object.values(project.sequences[sequence]?.tracks ?? {})
        .flatMap((track) => Object.values(track.items))
        .filter((item): item is ClipItem => item.kind === "clip");
      expect(clips.length).toBe(5000);
      const heavy = [...clips].sort((a, b) => keyCount(b) - keyCount(a));

      for (const count of [1, 8]) {
        const items = heavy.slice(0, count).map((clip) => clip.id);
        const cache = new CompiledCache();
        const inputs = {
          project,
          sequence,
          items,
          hidden: [],
          label: (prop: string) => prop,
          clipLabel: (item: string) => item,
        };
        const b0 = performance.now();
        const built = buildCurves(inputs, cache);
        const buildMs = performance.now() - b0;
        const keys = built.curves.reduce((n, c) => n + c.times.length, 0);
        expect(built.curves.length).toBeGreaterThan(0);

        const view = new GraphView(
          {
            width: 0,
            height: 0,
            style: {},
            getContext: () => noopContext,
            setAttribute: () => undefined,
          } as unknown as HTMLCanvasElement,
          { second: F, frame: F / 30, formatTime: String, formatValue: String },
          { theme: THEME, frames: { request: () => 1, cancel: () => undefined } },
        );
        view.setCurves(built.curves);
        view.resize(1600, 500, 1);
        const fitted = view.viewport;
        if (fitted === null) throw new Error("nothing to fit");
        const spanT = fitted.t1 - fitted.t0;
        const spanV = fitted.v1 - fitted.v0;
        const drawMs: number[] = [];
        // From everything in view to a tenth of it, panning across the keys and the values.
        for (let step = 0; step < 240; step++) {
          const z = 0.55 - 0.45 * Math.cos((2 * Math.PI * step) / 80);
          const t0 = fitted.t0 + spanT * (1 - z) * (0.5 + 0.5 * Math.sin(step / 17));
          const v0 = fitted.v0 + spanV * 0.1 * Math.sin(step / 9);
          view.setViewport({ t0, t1: t0 + spanT * z, v0, v1: v0 + spanV });
          view.setPlayhead(t0 + (spanT * z * step) / 240);
          const d0 = performance.now();
          view.flush();
          drawMs.push(performance.now() - d0);
        }

        // A patch that does not touch the selected clips: the same clip objects, nothing to do.
        const elsewhere: Project = { ...project, meta: { ...project.meta } };
        const c0 = performance.now();
        let same = true;
        for (let i = 0; i < 100; i++) {
          same = same && sameClips(built, selectedClips(elsewhere, sequence, items));
        }
        const checkMs = (performance.now() - c0) / 100;
        expect(same).toBe(true);

        console.info(
          `P-5K graph editor, ${count} clip(s): ${built.curves.length} curves, ${keys} keys; ` +
            `build ${buildMs.toFixed(2)} ms, frame p95 ${p95(drawMs).toFixed(2)} ms ` +
            `(sampling and paths at 1600 px, no rasterizing), ` +
            `an unrelated patch ${checkMs.toFixed(4)} ms`,
        );
        // 55 frames per second leave 18 ms a frame; the editor's own share must be a fraction.
        expect(p95(drawMs)).toBeLessThan(9);
        expect(checkMs).toBeLessThan(1);
        cache.dispose();
      }
      expect(math().liveAnimatables()).toBe(0);
    },
  );
});
