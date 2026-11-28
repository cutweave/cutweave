// SPDX-License-Identifier: GPL-3.0-or-later
// F61 Timeline tools, selection and track headers (docs/user-flows.md, WP-2A.10): the razor from
// the toolbar, V for the select tool, the zoom buttons, a marquee over two clips and their
// sounds, the toolbar's linked-selection toggle (on, a drag of the picture moves its sound; off,
// the picture moves alone), copy and paste onto another track, M, I and O, the lock button in a
// track header (a drag on the locked track is refused with a toast), Esc in the middle of a drag,
// and Ctrl+Z after the edits. A 100-step drag reads the app's own round-trip probe (User Timing
// `cutweave.timeline.rtt`, UI to engine to patch to UI): p95 under 16 ms. A drag paced like a hand
// (moves that do not wait for the app) keeps the app drawing and the clip keeping up (issue #241:
// every update re-rendered the whole window; `expectKeptUp`). Every timeline menu
// (clip, edge, lane, track, ruler, playhead, marker) is opened with a right-click and passes
// axe. Real clicks, drags and keys; only the OS file picker is stubbed.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { Locator, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  clipAt,
  clipBox,
  createProject,
  describePacedDrag,
  drag,
  dropCard,
  editorCaughtUp,
  expectKeptUp,
  freeMainTrack,
  handPacedDrag,
  hasRealEngine,
  importWithButton,
  laneY,
  launchFlowApp,
  layout,
  movePlayhead,
  rulerY,
  sec,
  sequence,
  shortcut,
  showTimeline,
  snapshot,
  type TimelineGeometry,
  timelineCanvas,
  timelineGeometry,
  timelineTools,
  toast,
  toolbar,
  workspace,
  xAt,
} from "./helpers.ts";

const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");

/** axe's findings for the open menu, as `rule: first node` (the editor's own axe-core). */
async function menuViolations(page: Page): Promise<string[]> {
  const source = readFileSync(
    createRequire(path.join(desktopDir, "..", "..", "packages", "editor", "package.json")).resolve(
      "axe-core/axe.min.js",
    ),
    "utf8",
  );
  await page.evaluate(source);
  return page.evaluate(async () => {
    interface AxeResult {
      violations: { id: string; nodes: { html: string; failureSummary?: string }[] }[];
    }
    const axe = (globalThis as unknown as { axe: { run(context: Element): Promise<AxeResult> } })
      .axe;
    const menu = document.querySelector('[role="menu"]');
    if (menu === null) return ["no menu is open"];
    // The menu fades in: its colours are the ones the person reads once it has.
    await Promise.all(menu.getAnimations({ subtree: true }).map((a) => a.finished));
    const results = await axe.run(menu);
    return results.violations.map(
      (v) => `${v.id}: ${v.nodes[0]?.html ?? ""} ${v.nodes[0]?.failureSummary ?? ""}`,
    );
  });
}

/** The selection as the engine's view has it. */
const selection = async (page: Page) =>
  (await editorCall<{ selection: { kind: string; id: string }[] }>(page, "view.get")).selection;

/** The round trips the app measured, ms (read-only: the standard User Timing entries). */
const roundTrips = (page: Page) =>
  page.evaluate(() =>
    performance.getEntriesByName("cutweave.timeline.rtt", "measure").map((e) => e.duration),
  );

/** V1's clips as `[start, duration]` seconds (the track added later goes above it). */
const v1Clips = async (page: Page): Promise<[number, number][]> =>
  (await layout(page)).find((t) => t.track === "V1")?.clips ?? [];

const p95 = (values: readonly number[]): number => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil(0.95 * sorted.length) - 1)] ?? Number.NaN;
};

/** A point inside the lane area, never over the headers or past the canvas. */
function inLanes(g: TimelineGeometry, x: number): number {
  return Math.min(Math.max(x, g.box.x + g.headerWidth + 4), g.box.x + g.box.width - 4);
}

test.describe("F61 timeline tools, selection and track headers", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("razor, marquee, linked selection, paste on another track, markers, the lock, Esc, undo; every menu passes axe", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await freeMainTrack(app, page); // clips stay where they are dropped
      await createProject(app, page, "Tools", work.dir);
      await importWithButton(app, page, [
        work.file("interview.mp4"),
        work.file("broll-city.mp4"),
        work.file("logo.png"),
      ]);
      await dropCard(page, "interview.mp4", "V1", 1);
      const both = (v1: [number, number][], a1: [number, number][] = v1) => [
        { track: "V1", clips: v1 },
        { track: "A1", clips: a1 },
      ];
      await expect.poll(() => layout(page)).toEqual(both([[1, 10]]));
      await editorCaughtUp(page);
      const tools = timelineTools(page);
      const tool = (name: string): Locator => tools.getByRole("button", { name, exact: true });
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });
      const canvas = timelineCanvas(page).locator("canvas");

      // The razor, from the toolbar: a click on the clip at 4 s cuts it and its sound there.
      await tool("Razor").click();
      await expect(tool("Razor")).toHaveAttribute("aria-pressed", "true");
      let g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(4)), laneY(g, "V1"));
      await expect
        .poll(() => layout(page))
        .toEqual(
          both([
            [1, 3],
            [4, 7],
          ]),
        );
      await editorCaughtUp(page);
      // Ctrl+Z restores the clip in one step.
      await shortcut(app, "Control+Z");
      await expect.poll(() => layout(page)).toEqual(both([[1, 10]]));
      await editorCaughtUp(page);
      // V, on the focused timeline, is the select tool again.
      await canvas.focus();
      await page.keyboard.press("v");
      await expect(tool("Select")).toHaveAttribute("aria-pressed", "true");
      await expect(tool("Razor")).toHaveAttribute("aria-pressed", "false");

      // The zoom buttons (view state, no undo): Zoom out twice is 0.8 × 0.8, Zoom in once back to
      // 0.8; then Zoom out until the first 32 s show.
      const zoom = async () =>
        (await editorCall<{ zoom: number | null }>(page, "view.get")).zoom ?? 100;
      await tool("Zoom out").click();
      await tool("Zoom out").click();
      await expect.poll(zoom).toBeCloseTo(64, 6);
      await tool("Zoom in").click();
      await expect.poll(zoom).toBeCloseTo(80, 6);
      await showTimeline(page, 32);

      // A marquee from the empty lane before the clips, across the B-roll and both sounds:
      // the two pictures and the two sounds are selected.
      await movePlayhead(page, 14);
      await dropCard(page, "broll-city.mp4", "V1", 14);
      await expect
        .poll(() => layout(page))
        .toEqual(
          both([
            [1, 10],
            [14, 10],
          ]),
        );
      await editorCaughtUp(page);
      g = await timelineGeometry(page);
      await drag(
        page,
        { x: inLanes(g, xAt(g, sec(0.3))), y: laneY(g, "V1") - 10 },
        { x: inLanes(g, xAt(g, sec(20))), y: laneY(g, "A1") + 10 },
      );
      await expect.poll(async () => (await selection(page)).length).toBe(4);
      await snapshot(page, info, "marquee selection");

      // Linked selection is on: a click takes the picture and its sound, a drag moves both.
      const link = tool("Linked selection");
      await expect(link).toHaveAttribute("aria-pressed", "true");
      g = await timelineGeometry(page);
      const interview = clipAt(g, "V1", 1).id;
      let box = clipBox(g, interview);
      await page.mouse.click(box.x + 30, box.y);
      await expect.poll(async () => (await selection(page)).length).toBe(2);
      await drag(page, { x: box.x + 30, y: box.y }, { x: box.x + 30 + g.pxPerSec, y: box.y });
      await expect
        .poll(() => layout(page))
        .toEqual(
          both([
            [2, 10],
            [14, 10],
          ]),
        );
      await expect(undo).toHaveAccessibleName("Undo Move clip");
      await editorCaughtUp(page);
      // Off: a click takes the picture alone, and its drag leaves the sound where it was.
      await link.click();
      await expect(link).toHaveAttribute("aria-pressed", "false");
      g = await timelineGeometry(page);
      box = clipBox(g, interview);
      await page.mouse.click(box.x + 30, box.y);
      await expect.poll(async () => (await selection(page)).length).toBe(1);
      await drag(page, { x: box.x + 30, y: box.y }, { x: box.x + 30 + g.pxPerSec, y: box.y });
      await expect
        .poll(() => layout(page))
        .toEqual(
          both(
            [
              [3, 10],
              [14, 10],
            ],
            [
              [2, 10],
              [14, 10],
            ],
          ),
        );
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect
        .poll(() => layout(page))
        .toEqual(
          both([
            [2, 10],
            [14, 10],
          ]),
        );
      await editorCaughtUp(page);
      await link.click();
      await expect(link).toHaveAttribute("aria-pressed", "true");

      // Copy the logo and paste it on another video track: Add track > Video track in the
      // corner above the headers, click the new track's header, move the playhead, Ctrl+V.
      await movePlayhead(page, 26);
      await dropCard(page, "logo.png", "V1", 26);
      await expect
        .poll(async () => (await layout(page))[0]?.clips)
        .toEqual([
          [2, 10],
          [14, 10],
          [26, 5],
        ]);
      await editorCaughtUp(page);
      await page.getByRole("button", { name: "Add track", exact: true }).click();
      await page.getByRole("menuitem", { name: "Video track" }).click();
      await expect.poll(async () => (await sequence(page)).tracks.length).toBe(3);
      await editorCaughtUp(page);
      const added = (await sequence(page)).tracks.find(
        (t) => t.name !== "V1" && t.kind === "video",
      );
      if (added === undefined) throw new Error("no new video track");
      g = await timelineGeometry(page);
      const logo = clipAt(g, "V1", 26).id;
      box = clipBox(g, logo);
      await page.mouse.click(box.x + 10, box.y);
      await shortcut(app, "Control+C");
      await page.mouse.click(g.box.x + 12, laneY(g, added.name));
      await expect.poll(async () => (await selection(page)).map((r) => r.kind)).toEqual(["track"]);
      await movePlayhead(page, 5);
      await shortcut(app, "Control+V");
      await expect
        .poll(async () => (await layout(page)).find((t) => t.track === added.name)?.clips)
        .toEqual([[5, 5]]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect
        .poll(async () => (await layout(page)).find((t) => t.track === added.name)?.clips)
        .toEqual([]);
      await editorCaughtUp(page);

      // M adds a marker at the playhead; I and O set the range around it (from the timeline).
      await canvas.focus();
      await page.keyboard.press("m");
      const markers = async () =>
        (
          await editorCall<{ markers: { id: string; t: number }[] }>(
            page,
            "timeline.list_markers",
            {
              sequence: (await sequence(page)).id,
            },
          )
        ).markers.map((m) => m.t);
      await expect.poll(markers).toEqual([sec(5)]);
      await page.keyboard.press("i");
      g = await timelineGeometry(page);
      await movePlayhead(page, 9);
      await canvas.focus();
      await page.keyboard.press("o");
      const inOut = async () =>
        (
          await editorCall<{ in_out?: { start: number; end: number } | null }>(
            page,
            "timeline.list_markers",
            { sequence: (await sequence(page)).id },
          )
        ).in_out ?? null;
      await expect.poll(inOut).toEqual({ start: sec(5), end: sec(9) });
      await editorCaughtUp(page);
      await snapshot(page, info, "marker and in out");

      // The lock button in V1's header: a drag of its clip is refused, with a toast; unlock.
      await page.getByRole("button", { name: "Lock track V1" }).click();
      const unlock = page.getByRole("button", { name: "Unlock track V1" });
      await expect(unlock).toHaveAttribute("aria-pressed", "true");
      const before = await layout(page);
      g = await timelineGeometry(page);
      box = clipBox(g, interview);
      await drag(page, { x: box.x + 30, y: box.y }, { x: box.x + 30 + g.pxPerSec, y: box.y });
      await expect(toast(page, "Clip not moved")).toBeVisible();
      expect(await layout(page)).toEqual(before);
      await unlock.click();
      await expect(page.getByRole("button", { name: "Lock track V1" })).toHaveAttribute(
        "aria-pressed",
        "false",
      );
      await editorCaughtUp(page);

      // Esc in the middle of a drag: the B-roll goes back, nothing to undo.
      const label = await undo.getAttribute("aria-label");
      g = await timelineGeometry(page);
      box = clipBox(g, clipAt(g, "V1", 14).id);
      await page.mouse.move(box.x + 30, box.y);
      await page.mouse.down();
      await page.mouse.move(box.x + 34, box.y, { steps: 2 });
      await page.mouse.move(box.x + 30 + 2 * g.pxPerSec, box.y, { steps: 8 });
      await expect.poll(async () => (await v1Clips(page))[1]?.[0]).toBe(16);
      await page.keyboard.press("Escape");
      await page.mouse.up();
      await expect.poll(async () => (await v1Clips(page))[1]?.[0]).toBe(14);
      await editorCaughtUp(page);
      await expect(undo).toHaveAttribute("aria-label", label ?? "");

      // A 100-step drag of the interview: the app's probe times every update's round trip.
      g = await timelineGeometry(page);
      box = clipBox(g, interview);
      await page.mouse.move(box.x + 30, box.y);
      await page.mouse.down();
      for (let step = 1; step <= 100; step += 1) {
        await page.mouse.move(box.x + 30 + step, box.y);
      }
      await page.mouse.up();
      await editorCaughtUp(page);
      const rtt = await roundTrips(page);
      console.info(
        `RTT over a 100-step drag: ${rtt.length} updates, p95 ${p95(rtt).toFixed(2)} ms, max ${Math.max(...rtt).toFixed(2)} ms`,
      );
      expect(rtt.length).toBeGreaterThan(5);
      expect(p95(rtt)).toBeLessThan(16);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await v1Clips(page))[0]).toEqual([2, 10]);
      await editorCaughtUp(page);

      // A drag paced like a hand (issue #241): the pointer moves every 16 ms whether or not the
      // app has caught up with the last move, out to the right (short of the B-roll) and back,
      // twice, and lets go where it started. The app keeps drawing (frame intervals p95 under
      // 50 ms), the clip keeps up (20 updates or more) and the round trips stay short (p95 under
      // 100 ms); the clip ends where it was, with nothing to undo.
      const undoBefore = await undo.getAttribute("aria-label");
      g = await timelineGeometry(page);
      box = clipBox(g, interview);
      const paced = await handPacedDrag(page, { x: box.x + 30, y: box.y }, 1.8 * g.pxPerSec);
      console.info(describePacedDrag(paced));
      expectKeptUp(paced);
      await expect.poll(async () => (await v1Clips(page))[0]).toEqual([2, 10]);
      await editorCaughtUp(page);
      await expect(undo).toHaveAttribute("aria-label", undoBefore ?? "");

      // Every timeline menu, opened as a person does, passes axe.
      g = await timelineGeometry(page);
      box = clipBox(g, interview);
      const playheadX = xAt(g, (await editorCall<{ playhead: number }>(page, "view.get")).playhead);
      const markerX = xAt(g, sec(5));
      const menus: [string, { x: number; y: number }][] = [
        ["Clip actions for interview.mp4", { x: box.x + 40, y: box.y }],
        ["Edge actions for interview.mp4", { x: box.x + 2, y: box.y }],
        ["Timeline actions", { x: inLanes(g, xAt(g, sec(0.5))), y: laneY(g, "V1") }],
        ["Track actions for V1", { x: g.box.x + 12, y: laneY(g, "V1") }],
        ["Ruler actions", { x: inLanes(g, xAt(g, sec(20))), y: rulerY(g) }],
        ["Playhead actions", { x: playheadX, y: rulerY(g) }],
        ["Marker actions for", { x: markerX, y: g.box.y + g.rulerHeight - 3 }],
      ];
      for (const [name, at] of menus) {
        await page.mouse.click(at.x, at.y, { button: "right" });
        const menu = page.getByRole("menu", { name: new RegExp(`^${name}`) });
        await expect(menu, name).toBeVisible();
        expect(await menuViolations(page), name).toEqual([]);
        await page.keyboard.press("Escape");
        await expect(page.getByRole("menu")).toHaveCount(0);
      }
    } finally {
      await close();
      work.remove();
    }
  });
});
