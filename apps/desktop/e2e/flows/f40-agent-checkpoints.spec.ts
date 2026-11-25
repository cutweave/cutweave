// SPDX-License-Identifier: GPL-3.0-or-later
// F40 An agent rolls back to a checkpoint (docs/user-flows.md, WP-2I.03): the person drops a
// clip; an agent attached through the `cutweave` CLI (as `agent:cli`) saves a checkpoint,
// deletes the clip and restores the checkpoint. The person watches the clip go and come back,
// sees the restore named on the Undo button, and takes it back and forth with Ctrl+Z / Ctrl+Y.
// Then the engine dies (fault injection, as in F11) and comes back: the checkpoint is still
// there, and restoring it brings the clip back once more.
import { execFile } from "node:child_process";
import path from "node:path";
import { promisify } from "node:util";
import type { ElectronApplication, Page } from "@playwright/test";
import { engineStatus, expect, killEngine, test } from "../fixtures.ts";
import {
  cliBin,
  createProject,
  dropCard,
  editorCaughtUp,
  engineChip,
  hasCli,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  layout,
  sequence,
  shortcut,
  snapshot,
  toast,
  toolbar,
  workspace,
} from "./helpers.ts";

const run = promisify(execFile);

/** `cutweave exec <step>` attached to the app's current engine; resolves with the output. */
async function agent(
  app: ElectronApplication,
  home: string,
  step: { cmd: string; args: Record<string, unknown> },
): Promise<Record<string, unknown>> {
  const pid = await app.evaluate(() => globalThis.__cutweaveTest?.enginePid() ?? null);
  if (pid === null) throw new Error("no engine is running");
  // Dev stub media in the CLI's own environment would make it spawn an engine of its own.
  const { CUTWEAVE_DEV_STUB_MEDIA: _stub, ...inherited } = process.env;
  const env = { ...inherited, HOME: home, USERPROFILE: home };
  const { stdout } = await run(
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
  return JSON.parse(stdout) as Record<string, unknown>;
}

/** Every clip id on the timeline (the video and its linked audio). */
async function clipIds(page: Page): Promise<string[]> {
  const seq = await sequence(page);
  return seq.tracks.flatMap((t) => t.items.map((i) => i.id));
}

const PLACED = [
  { track: "V1", clips: [[1, 10]] },
  { track: "A1", clips: [[1, 10]] },
];
const EMPTY = [
  { track: "V1", clips: [] },
  { track: "A1", clips: [] },
];

test.describe("F40 an agent rolls back to a checkpoint", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  test.skip(!hasCli, "the cutweave CLI is not built (cargo build -p cutweave-cli)");

  test("the agent's cleanup comes back from its checkpoint, also after an engine crash", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, home, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Checkpoints", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(() => layout(page)).toEqual(PLACED);
      await editorCaughtUp(page);

      // The agent saves a checkpoint before its risky step, then "cleans up" the timeline.
      const made = await agent(app, home, {
        cmd: "edit.checkpoint",
        args: { name: "before cleanup", note: "about to tidy the timeline" },
      });
      expect((made as { output?: { checkpoint?: string } }).output?.checkpoint).toBe("cp_1");
      await agent(app, home, { cmd: "item.delete", args: { items: await clipIds(page) } });
      await expect.poll(() => layout(page)).toEqual(EMPTY);
      await editorCaughtUp(page);

      // It changes its mind: the restore brings the clip back, as one step the person can undo.
      await agent(app, home, { cmd: "edit.restore", args: { checkpoint: "before cleanup" } });
      await expect.poll(() => layout(page)).toEqual(PLACED);
      await editorCaughtUp(page);
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });
      await expect(undo).toHaveAccessibleName(/^Undo Restore checkpoint .before cleanup. \(cli\)$/);
      await snapshot(page, info, "restored by the agent");
      await shortcut(app, "Control+Z");
      await expect.poll(() => layout(page)).toEqual(EMPTY);
      await shortcut(app, "Control+Y");
      await expect.poll(() => layout(page)).toEqual(PLACED);
      await editorCaughtUp(page);

      // The engine dies and comes back with the journal: the checkpoint survived it.
      await killEngine(app);
      await expect(toast(page, "Engine restarted — recovered")).toContainText(
        "Checkpoints.cutweave",
      );
      await expect(engineChip(page)).toHaveText("Engine connected");
      await expect.poll(async () => (await engineStatus(app))?.generation).toBe(2);
      const listed = (await agent(app, home, { cmd: "edit.list_checkpoints", args: {} })) as {
        output?: { checkpoints?: { name?: string }[] };
      };
      expect(listed.output?.checkpoints?.map((c) => c.name)).toEqual(["before cleanup"]);
      await agent(app, home, { cmd: "item.delete", args: { items: await clipIds(page) } });
      await expect.poll(() => layout(page)).toEqual(EMPTY);
      // An autosave may have run before the crash: the restore then reverts changes the
      // recovered history no longer holds, which takes `force`.
      await agent(app, home, {
        cmd: "edit.restore",
        args: { checkpoint: "before cleanup", force: true },
      });
      await expect.poll(() => layout(page)).toEqual(PLACED);
      await editorCaughtUp(page);
      await snapshot(page, info, "restored after the crash");
    } finally {
      await close();
      work.remove();
    }
  });
});
