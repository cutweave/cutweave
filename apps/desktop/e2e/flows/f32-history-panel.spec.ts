// SPDX-License-Identifier: GPL-3.0-or-later
// F32 History panel (docs/user-flows.md, WP-2H.11): the person drops a clip and makes four edits
// (insert, move, opacity, trim) and an agent attached through the `cutweave` CLI trims it. Edit >
// History lists the import, the four edits and the agent's trim, the agent's row named and in
// agent blue. Clicking the move's row takes the timeline back to the state after the move and
// dims the three later rows; Ctrl+Y redoes the opacity change; a new edit removes the rows that
// are still dimmed.
import { execFile } from "node:child_process";
import path from "node:path";
import { promisify } from "node:util";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  cliBin,
  clickClip,
  clipAt,
  createProject,
  drag,
  dropCard,
  editorCaughtUp,
  hasCli,
  hasRealEngine,
  importWithButton,
  inspector,
  item,
  laneY,
  launchFlowApp,
  layout,
  sec,
  sequence,
  shortcut,
  snapshot,
  timelineGeometry,
  workspace,
  xAt,
} from "./helpers.ts";

const run = promisify(execFile);

/** `cutweave exec <step>` attached to the app's current engine, as an agent would run it. */
async function agent(
  app: ElectronApplication,
  home: string,
  step: { cmd: string; args: Record<string, unknown> },
): Promise<void> {
  const pid = await app.evaluate(() => globalThis.__cutweaveTest?.enginePid() ?? null);
  if (pid === null) throw new Error("no engine is running");
  // Dev stub media in the CLI's own environment would make it spawn an engine of its own.
  const { CUTWEAVE_DEV_STUB_MEDIA: _stub, ...inherited } = process.env;
  const env = { ...inherited, HOME: home, USERPROFILE: home };
  await run(
    cliBin,
    [
      "exec",
      "--run-dir",
      path.join(home, ".cutweave", "run"),
      "--pid",
      String(pid),
      JSON.stringify(step),
    ],
    { env, timeout: 30_000 },
  );
}

/** The History panel's rows, oldest first. */
const rows = (page: Page): Locator =>
  page.getByRole("list", { name: "Project history" }).getByRole("button");

/** The row's name and who made it: "Move clip, You". */
async function labels(page: Page): Promise<string[]> {
  const all = await rows(page).evaluateAll((els) =>
    els.map((el) => el.getAttribute("aria-label") ?? ""),
  );
  return all.map((l) => l.split(", ").slice(0, 2).join(", "));
}

test.describe("F32 history panel", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  test.skip(!hasCli, "the cutweave CLI is not built (cargo build -p cutweave-cli)");

  test("lists every change and who made it, goes back to any step and forward again", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, home, close } = await launchFlowApp(work);
    try {
      const clips = async () => (await layout(page))[0]?.clips ?? [];
      const opacity = async () => {
        const id = (await sequence(page)).tracks[0]?.items[0]?.id;
        return id === undefined ? undefined : (await item(page, id)).visual.opacity;
      };

      // Import a clip and drop it at 1 s.
      await createProject(app, page, "History", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      const id = (await sequence(page)).tracks[0]?.items[0]?.id ?? "";
      // Zoom out so the moved clip's end stays in view, then drag it to 3 s.
      await clickClip(page, id);
      await page.keyboard.press("-");
      await page.keyboard.press("-");
      await expect.poll(async () => (await timelineGeometry(page)).pxPerSec).toBeCloseTo(64, 6);
      let g = await timelineGeometry(page);
      const px = (s: number) => s * g.pxPerSec;
      await drag(
        page,
        { x: xAt(g, clipAt(g, "V1", 1).start) + px(1), y: laneY(g, "V1") },
        { x: xAt(g, clipAt(g, "V1", 1).start) + px(3), y: laneY(g, "V1") },
      );
      await expect.poll(clips).toEqual([[3, 10]]);
      // One step down on the inspector's opacity slider.
      await inspector(page).opacity.focus();
      await page.keyboard.press("ArrowLeft");
      await expect.poll(opacity).toBe(0.99);
      // Trim it to 5 s by dragging its right edge.
      g = await timelineGeometry(page);
      const end = xAt(g, clipAt(g, "V1", 3).start + clipAt(g, "V1", 3).duration);
      await drag(
        page,
        { x: end - 2, y: laneY(g, "V1") },
        { x: end - 2 - px(5), y: laneY(g, "V1") },
      );
      await expect.poll(clips).toEqual([[3, 5]]);
      await editorCaughtUp(page);

      // An agent trims the clip's end back to 7 s.
      await agent(app, home, { cmd: "clip.trim", args: { item: id, edge: "end", to: sec(7) } });
      await expect.poll(clips).toEqual([[3, 4]]);
      await editorCaughtUp(page);

      // Edit > History opens the panel as a tab next to the Inspector.
      await chooseMenu(page, app, "Edit", "History");
      const panel = page.getByRole("region", { name: "History" });
      await expect(panel).toBeVisible();
      await expect
        .poll(() => labels(page))
        .toEqual([
          "Import media, You",
          "Insert clip, You",
          "Move clip, You",
          "Change opacity, You",
          "Trim clip, You",
          "Trim clip, cli",
        ]);
      // The agent's row is named and in agent blue; the person's is not.
      const agentColor = (row: Locator) =>
        row.locator(".cw-history__who").evaluate((el) => {
          const probe = document.createElement("span");
          probe.style.color = "var(--agent-text)";
          document.body.append(probe);
          const want = getComputedStyle(probe).color;
          probe.remove();
          return getComputedStyle(el).color === want;
        });
      expect(await agentColor(rows(page).nth(5))).toBe(true);
      expect(await agentColor(rows(page).nth(4))).toBe(false);
      await expect(rows(page).nth(5)).toHaveAttribute("data-actor", "agent");
      await expect(page.locator('[data-state="undone"]')).toHaveCount(0);
      await snapshot(page, info, "mixed history");

      // Click the move's row: the timeline is back after the move, and three rows are dimmed.
      await rows(page).nth(2).click();
      await expect.poll(clips).toEqual([[3, 10]]);
      expect(await opacity()).toBe(1);
      await expect(page.locator('[data-state="undone"]')).toHaveCount(3);
      await expect(rows(page).nth(3)).toHaveAttribute("data-state", "undone");
      await snapshot(page, info, "back at the move");

      // Ctrl+Y redoes the opacity change, one step at a time.
      await shortcut(app, "Control+Y");
      await expect.poll(opacity).toBe(0.99);
      await expect.poll(clips).toEqual([[3, 10]]);
      await expect(page.locator('[data-state="undone"]')).toHaveCount(2);

      // A new edit removes the rows that were still dimmed.
      g = await timelineGeometry(page);
      await drag(
        page,
        { x: xAt(g, clipAt(g, "V1", 3).start) + px(1), y: laneY(g, "V1") },
        { x: xAt(g, clipAt(g, "V1", 3).start) + px(2), y: laneY(g, "V1") },
      );
      await expect.poll(clips).toEqual([[4, 10]]);
      await expect(page.locator('[data-state="undone"]')).toHaveCount(0);
      await expect
        .poll(() => labels(page))
        .toEqual([
          "Import media, You",
          "Insert clip, You",
          "Move clip, You",
          "Change opacity, You",
          "Move clip, You",
        ]);
      await snapshot(page, info, "after a new edit");

      // The header's sort button lists the steps newest first (#171); it is remembered per user.
      await page.getByRole("button", { name: "Show newest first" }).click();
      await expect
        .poll(() => labels(page))
        .toEqual([
          "Move clip, You",
          "Change opacity, You",
          "Move clip, You",
          "Insert clip, You",
          "Import media, You",
        ]);
      // Going back to "Change opacity" dims what came after it, above the marker.
      await rows(page).nth(1).click();
      await expect.poll(clips).toEqual([[3, 10]]);
      await expect(rows(page).nth(0)).toHaveAttribute("data-state", "undone");
      await expect(rows(page).filter({ hasText: "Change opacity" })).toHaveAttribute(
        "data-state",
        "applied",
      );
      const marker = page.locator(".cw-history__marker");
      await expect(marker.locator("xpath=preceding-sibling::li[1]//button")).toHaveAttribute(
        "data-state",
        "undone",
      );
      // The arrow keys follow the list on screen: Down is the next older step.
      const opacityRow = rows(page).filter({ hasText: "Change opacity" });
      await opacityRow.focus();
      await page.keyboard.press("ArrowDown");
      await expect(rows(page).filter({ hasText: "Move clip" }).last()).toBeFocused();
      await page.getByRole("button", { name: "Show oldest first" }).click();
      await expect
        .poll(async () => (await labels(page)).slice(0, 4))
        .toEqual([
          "Import media, You",
          "Insert clip, You",
          "Move clip, You",
          "Change opacity, You",
        ]);
      await expect(rows(page).nth(5)).toHaveAttribute("data-state", "undone");
    } finally {
      await close();
      work.remove();
    }
  });
});
