// SPDX-License-Identifier: GPL-3.0-or-later
// F44 An agent opens a project while the person has unsaved work (docs/user-flows.md; ADR-0029,
// issue #161). The app starts on an untitled document and the person imports a file into it.
// The `cutweave` CLI, attached to the app's engine as `agent:cli`, asks to open another project:
// the engine refuses, the work stays and a toast in agent colour says why. The person saves; the
// agent asks again, and the app shows the project opening (who opens it), then the project.
import { execFile } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { promisify } from "node:util";
import type { ElectronApplication } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  cliBin,
  createProject,
  dirtyDot,
  hasCli,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  mediaCards,
  namingDialog,
  profileDir,
  projectDoc,
  removeDir,
  shortcut,
  snapshot,
  toast,
  workspace,
} from "./helpers.ts";

const run = promisify(execFile);

interface ExecOutcome {
  /** The CLI's exit code: 0 ok, 1 the command failed. */
  readonly code: number;
  /** The result (stdout) when it succeeded, the engine's error (stderr) when it failed. */
  readonly json: Record<string, unknown>;
}

/**
 * `cutweave exec <step>` attached to the app's engine, as an agent runs it: through the run-file
 * in the app's HOME, the engine chosen by its pid. Resolves with the exit code and the JSON the
 * CLI printed, whether the command succeeded or not.
 */
async function agentExec(
  app: ElectronApplication,
  home: string,
  step: { cmd: string; args: Record<string, unknown> },
): Promise<ExecOutcome> {
  const pid = await app.evaluate(() => globalThis.__cutweaveTest?.enginePid() ?? null);
  if (pid === null) throw new Error("no engine is running");
  // Dev stub media in the CLI's own environment would make it spawn an engine of its own.
  const { CUTWEAVE_DEV_STUB_MEDIA: _stub, ...inherited } = process.env;
  const env = { ...inherited, HOME: home, USERPROFILE: home };
  const args = [
    "exec",
    "--run-dir",
    path.join(home, ".cutweave", "run"),
    "--pid",
    String(pid),
    JSON.stringify(step),
  ];
  try {
    const { stdout } = await run(cliBin, args, { env, timeout: 30_000 });
    return { code: 0, json: JSON.parse(stdout) as Record<string, unknown> };
  } catch (error) {
    const failed = error as { code?: unknown; stderr?: string };
    if (typeof failed.code !== "number" || failed.stderr === undefined) throw error;
    return { code: failed.code, json: JSON.parse(failed.stderr) as Record<string, unknown> };
  }
}

const mediaOnDisk = (doc: string): number =>
  Object.keys((JSON.parse(readFileSync(doc, "utf8")) as { media: object }).media).length;

test.describe("F44 an agent opens a project while the person has unsaved work", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  test.skip(!hasCli, "the cutweave CLI is not built (cargo build -p cutweave-cli)");

  test("refused while the work is unsaved; after a save it opens, and the app shows it opening", async () => {
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    const second = path.join(work.dir, "Second");
    try {
      // An earlier session made the project the agent will open.
      const earlier = await launchFlowApp(work, home);
      await createProject(earlier.app, earlier.page, "Second", work.dir);
      await earlier.close();

      // The app starts on an untitled document, and the person starts working in it.
      const { app, page, close } = await launchFlowApp(work, home);
      try {
        await expect(page).toHaveTitle("Untitled — Cutweave");
        await importWithButton(app, page, [work.file("interview.mp4")]);
        await expect(page).toHaveTitle("• Untitled — Cutweave");

        // The agent asks to open Second: refused, with the reason and what to do.
        const refused = await agentExec(app, home, {
          cmd: "project.open",
          args: { path: second },
        });
        expect(refused.code).toBe(1);
        expect(refused.json).toMatchObject({ code: "conflict", reason: "unsaved_changes" });
        // The person's work is still there, and a toast in agent colour says why.
        const note = toast(page, "Project not opened");
        await expect(note).toHaveAttribute("data-tone", "agent");
        await expect(note).toContainText(
          "Command line (agent:cli) tried to open Second, but Untitled has unsaved changes. Save them, then ask the agent again.",
        );
        await expect(page).toHaveTitle("• Untitled — Cutweave");
        await expect(mediaCards(page)).toHaveCount(1);
        await snapshot(page, info, "agent open refused");

        // The person saves: an untitled document asks for a name and a place first.
        await shortcut(app, "Control+S");
        const naming = namingDialog(page, "Save project as");
        await expect(naming.dialog).toBeVisible();
        await naming.name.fill("First");
        await naming.location.fill(work.dir);
        await naming.create.click();
        await expect(naming.dialog).toBeHidden();
        // Saved in the folder First, and the project takes that name (#165): nothing is unsaved.
        await expect(page).toHaveTitle("First — Cutweave");
        await expect(dirtyDot(page)).toBeHidden();
        await note.getByRole("button", { name: "Dismiss" }).click();
        await expect(note).toBeHidden();

        // The agent asks again: the app shows Second opening, and who opens it, then Second.
        const opening = page
          .getByRole("dialog", { name: "Opening Second…" })
          .filter({ hasText: "Command line (agent:cli) is opening this project." });
        const done = toast(page, "Project opened by an agent");
        const [opened] = await Promise.all([
          agentExec(app, home, { cmd: "project.open", args: { path: second } }),
          opening.waitFor({ state: "visible", timeout: 30_000 }),
          done.waitFor({ state: "visible", timeout: 30_000 }),
        ]);
        expect(opened.code).toBe(0);
        await expect(opening).toBeHidden();
        await expect(page).toHaveTitle("Second — Cutweave");
        await expect(mediaCards(page)).toHaveCount(0);
        await expect(done).toHaveAttribute("data-tone", "agent");
        await expect(done).toContainText("Command line (agent:cli) opened Second.");
        await snapshot(page, info, "agent opened the project");
        // Nothing the person made was lost: First holds the imported file.
        expect(mediaOnDisk(projectDoc(work.dir, "First"))).toBe(1);
      } finally {
        await close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
