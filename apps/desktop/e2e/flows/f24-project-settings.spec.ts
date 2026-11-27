// SPDX-License-Identifier: GPL-3.0-or-later
// F24 Project settings (docs/user-flows.md, WP-2H.08): the settings saved in the project have
// their own dialog, File > Project settings… (also "Project settings…" in the Inspector when
// nothing is selected). Set the still image duration to 3 seconds and drop a picture on the
// timeline: its clip is 3 s. Ctrl+Z takes the clip back, then the setting: the open dialog shows
// 5 s again. Redo, "Use as defaults for new projects", and a project created afterwards starts
// at 3 s. An agent (`cutweave exec` with `project.set_settings`) changes the proxy policy and
// the open dialog shows it.
import { execFile } from "node:child_process";
import path from "node:path";
import { promisify } from "node:util";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  cliBin,
  createProject,
  dirtyDot,
  dropCard,
  editorCaughtUp,
  hasCli,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  profileDir,
  removeDir,
  sec,
  sequence,
  shortcut,
  workspace,
} from "./helpers.ts";

const run = promisify(execFile);
const BOUND = { timeout: 15_000 };

/** `cutweave exec <step>` attached to the app's engine (its run-file is in the app's HOME). */
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

const dialogOf = (page: Page): Locator => page.getByRole("dialog", { name: "Project settings" });
const stillField = (dialog: Locator): Locator =>
  dialog.getByRole("spinbutton", { name: "Still image duration" });

/** What the project stores, as an agent reads it. */
async function settings(page: Page): Promise<{ still_duration: number; proxy_policy: string }> {
  const info = await editorCall<{
    settings: { still_duration: number; proxy_policy: string };
  }>(page, "project.info", {});
  return info.settings;
}

/** The lengths of every clip of the sequence, in flicks. */
async function clipLengths(page: Page): Promise<number[]> {
  const seq = await sequence(page);
  return seq.tracks.flatMap((t) => t.items).map((i) => i.duration);
}

test.describe("F24 project settings", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  test.skip(!hasCli, "the cutweave CLI is not built (cargo build -p cutweave-cli)");

  test("the project's own settings: change, undo, defaults for new projects, an agent's change", async () => {
    test.setTimeout(240_000);
    const work = workspace();
    const home = profileDir();
    try {
      const { app, page, close } = await launchFlowApp(work, home);
      try {
        await createProject(app, page, "First", work.dir);
        await importWithButton(app, page, [work.file("logo.png")]);

        // Nothing is selected: the Inspector links to the project's settings.
        await page.getByRole("button", { name: "Project settings…" }).click();
        const dialog = dialogOf(page);
        await expect(dialog).toBeVisible(BOUND);
        await expect(stillField(dialog)).toHaveValue(/^5/);
        await dialog.getByRole("button", { name: "Close" }).click();
        await expect(dialog).toBeHidden();

        // File > Project settings…: 3 seconds for stills.
        await chooseMenu(page, app, "File", "Project settings…");
        await expect(dialog).toBeVisible(BOUND);
        await stillField(dialog).dblclick();
        await page.keyboard.type("3");
        await page.keyboard.press("Enter");
        await expect.poll(async () => (await settings(page)).still_duration, BOUND).toBe(sec(3));
        await expect(stillField(dialog)).toHaveValue(/^3/);
        await dialog.getByRole("button", { name: "Close" }).click();
        await expect(dialog).toBeHidden();

        // A picture dropped on the timeline is 3 s long.
        await dropCard(page, "logo.png", "V1", 0);
        await expect.poll(() => clipLengths(page), BOUND).toEqual([sec(3)]);
        await editorCaughtUp(page);

        // Undo takes the clip back, then the setting: the open dialog shows 5 s again.
        await chooseMenu(page, app, "File", "Project settings…");
        await expect(dialog).toBeVisible(BOUND);
        await dialog.getByRole("heading", { name: "Project settings" }).click();
        await shortcut(app, "Control+Z");
        await expect.poll(() => clipLengths(page), BOUND).toEqual([]);
        await expect(stillField(dialog)).toHaveValue(/^3/);
        await shortcut(app, "Control+Z");
        await expect(stillField(dialog)).toHaveValue(/^5/, BOUND);
        expect((await settings(page)).still_duration).toBe(sec(5));

        // Redo the setting, and make it the default for new projects.
        await shortcut(app, "Control+Y");
        await expect(stillField(dialog)).toHaveValue(/^3/, BOUND);
        await dialog.getByRole("button", { name: "Use as defaults for new projects" }).click();
        await expect(dialog.getByRole("status")).toContainText("3 s", BOUND);

        // An agent changes the proxy policy: the open dialog shows it.
        const all = dialog.getByRole("radio", { name: "All video" });
        await expect(all).toHaveAttribute("aria-checked", "false");
        await agent(app, home, {
          cmd: "project.set_settings",
          args: { patch: { proxy_policy: "all" } },
        });
        await expect(all).toHaveAttribute("aria-checked", "true", BOUND);
        await dialog.getByRole("button", { name: "Close" }).click();
        await expect(dialog).toBeHidden();

        // A project created now starts at 3 s (and with the app's proxy default, not the
        // agent's change to the first project).
        await shortcut(app, "Control+S");
        await expect(dirtyDot(page)).toBeHidden();
        await createProject(app, page, "Second", work.dir);
        await expect
          .poll(() => settings(page), BOUND)
          .toMatchObject({
            still_duration: sec(3),
            proxy_policy: "auto",
          });
        await chooseMenu(page, app, "File", "Project settings…");
        await expect(stillField(dialog)).toHaveValue(/^3/, BOUND);
        await expect(dialog.getByRole("radio", { name: "Auto" })).toHaveAttribute(
          "aria-checked",
          "true",
        );
        await dialog.getByRole("button", { name: "Close" }).click();
        await expect(dialog).toBeHidden();
      } finally {
        await close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
