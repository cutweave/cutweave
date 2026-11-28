// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.05 against the P-5K benchmark project (WP-2J.01: 5,000 items, 50,000 keyframes): the
// lanes of every clip are projected from the real document and laid out, and a frame with all
// of them open draws a bounded number of marks. Skips when the project has not been generated
// (`cargo xtask gen-bench-project`; `testdata/bench-projects/` is not committed).
//
// With `CUTWEAVE_P5K_OUT=<file>` it also writes the timeline model and the lanes as JSON, which
// the browser benchmark reads to time real canvas frames:
//   pnpm --filter @cutweave/timeline bench --lanes --fixture <file>
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Project, SequenceId } from "@cutweave/protocol";
import { KEY_COLLAPSE_PX, type Theme, TimelineView } from "@cutweave/timeline";
import { describe, expect, it } from "vitest";
import { selectTimelineModel } from "../state/selectors.ts";
import { EMPTY_KEY_SELECTION, laneGroups } from "./keyframes.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");
const FOLDER = join(ROOT, "testdata", "bench-projects", "P-5K");
const DOCUMENT = existsSync(FOLDER)
  ? readdirSync(FOLDER).find((name) => name.endsWith(".cutweave") && name !== ".cutweave")
  : undefined;

/** Token stand-ins: nothing is drawn, only the sizes matter. */
function theme(): Theme {
  const c = { strong: "var(--clip-media)", body: "var(--clip-media-body)" };
  return {
    surface0: "var(--surface-0)",
    surface1: "var(--surface-1)",
    surface3: "var(--surface-3)",
    lane: "var(--timeline-lane)",
    laneAlt: "var(--timeline-lane-alt)",
    borderSubtle: "var(--border-subtle)",
    textPrimary: "var(--text-primary)",
    textSecondary: "var(--text-secondary)",
    textMuted: "var(--text-muted)",
    playhead: "var(--playhead)",
    onMarigold: "var(--on-marigold)",
    marigold: "var(--marigold)",
    marigoldSubtle: "var(--marigold-subtle)",
    marigoldText: "var(--marigold-text)",
    agent: "var(--agent)",
    agentSubtle: "var(--agent-subtle)",
    clip: { media: c, audio: c, graphics: c, captions: c, effects: c },
    fontCaption: "var(--font-ui)",
    fontLabel: "var(--font-ui)",
    fontNumeric: "var(--font-mono)",
    captionPx: 11,
    numericPx: 12,
    opacityDisabled: 0.5,
    borderThin: 1,
    clipStrip: 3,
    radiusXs: 2,
    radiusSm: 4,
    controlSm: 24,
    controlMd: 28,
    iconSm: 14,
    trackSm: 32,
    trackMd: 48,
    trackLg: 72,
    space0_5: 2,
    space1: 4,
    space1_5: 6,
    space2: 8,
    space4: 16,
    space12: 48,
  };
}

const noopContext = new Proxy(
  {},
  { get: (_t, prop) => (prop === "measureText" ? () => ({ width: 0 }) : () => undefined) },
) as CanvasRenderingContext2D;

describe("keyframe lanes on the P-5K benchmark project", () => {
  it.skipIf(DOCUMENT === undefined)(
    "projects 50,000 keys into lanes and draws a bounded number of marks per frame",
    () => {
      const project = JSON.parse(readFileSync(join(FOLDER, DOCUMENT as string), "utf8")) as Project;
      const sequence = Object.keys(project.sequences)[0] as SequenceId;
      const model = selectTimelineModel(project, sequence);
      expect(model.items.length).toBe(5000);

      const open = model.items.map((item) => item.id);
      const t0 = performance.now();
      const lanes = laneGroups(project, sequence, open, EMPTY_KEY_SELECTION, (prop) => prop);
      const projectMs = performance.now() - t0;
      const keys = lanes.reduce(
        (n, group) => n + group.lanes.reduce((m, lane) => m + lane.times.length, 0),
        0,
      );
      expect(keys).toBe(50_000);

      const WIDTH = 1600;
      const view = new TimelineView(
        { width: 0, height: 0, style: {} } as unknown as HTMLCanvasElement,
        { theme: theme(), context: noopContext, requestFrame: () => 1, cancelFrame: () => {} },
      );
      view.resize(WIDTH, 900, 1);
      view.setModel(model);
      const t1 = performance.now();
      view.setKeyframeLanes(lanes);
      const layoutMs = performance.now() - t1;

      const end = Math.max(...model.items.map((item) => item.start + item.duration));
      const drawMs: number[] = [];
      let marksMax = 0;
      // From the whole project in view to a few clips per screen, across the project.
      for (let step = 0; step < 240; step++) {
        const z = 0.5 - 0.5 * Math.cos((2 * Math.PI * step) / 80);
        const pxPerSec = Math.exp(Math.log(0.25) + z * (Math.log(300) - Math.log(0.25)));
        view.setViewState({
          pxPerSec,
          scrollX: Math.round((end * (0.5 + 0.45 * Math.sin(step / 17))) / 2),
          scrollY: 200 * (0.5 - 0.5 * Math.cos(step / 9)),
        });
        view.invalidate();
        const d0 = performance.now();
        view.flush();
        drawMs.push(performance.now() - d0);
        marksMax = Math.max(marksMax, view.stats.visibleKeyframes);
        // Never more marks than a lane row can show, per row in view.
        expect(view.stats.visibleKeyframes).toBeLessThanOrEqual(
          view.stats.tracksDrawn * 8 * Math.ceil(WIDTH / KEY_COLLAPSE_PX),
        );
      }
      drawMs.sort((a, b) => a - b);
      const p95 = drawMs[Math.ceil(0.95 * drawMs.length) - 1] ?? 0;
      console.info(
        `P-5K lanes: ${lanes.length} clips, ${keys} keys; projection ${projectMs.toFixed(1)} ms, ` +
          `lane layout ${layoutMs.toFixed(1)} ms, draw p95 ${p95.toFixed(2)} ms (no canvas), ` +
          `marks max ${marksMax}`,
      );
      expect(marksMax).toBeGreaterThan(0);
      expect(marksMax).toBeLessThan(keys);

      const out = process.env.CUTWEAVE_P5K_OUT;
      if (out !== undefined && out !== "") {
        writeFileSync(
          out,
          JSON.stringify({
            model,
            lanes: lanes.map((group) => ({
              itemId: group.itemId,
              lanes: group.lanes.map((lane) => ({
                prop: lane.prop,
                label: lane.label,
                times: Array.from(lane.times),
                glyphs: Array.from(lane.glyphs),
              })),
            })),
          }),
        );
      }
    },
    120_000,
  );
});
