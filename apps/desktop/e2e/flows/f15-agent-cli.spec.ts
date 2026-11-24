// SPDX-License-Identifier: GPL-3.0-or-later
// F15 An agent edits while the UI is open (docs/user-flows.md): the `cutweave` CLI attaches to
// the engine the app runs (through its run-file, as the actor `agent:cli`), adds a clip and trims
// it. The UI shows each change live, and Ctrl+Z in the UI undoes it.
import { execFile } from "node:child_process";
import path from "node:path";
import { promisify } from "node:util";
import type { ElectronApplication, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  cliBin,
  clickClip,
  createProject,
  dirtyDot,
  editorCaughtUp,
  hasCli,
  hasRealEngine,
  importWithButton,
  inspector,
  launchFlowApp,
  layout,
  mediaCards,
  sec,
  sequence,
  shortcut,
  snapshot,
  toolbar,
  workspace,
} from "./helpers.ts";

const run = promisify(execFile);

/**
 * `cutweave exec <step>` attached to the app's engine: its run-file is in the app's HOME, and
 * the engine's pid picks it. Resolves with the step's output.
 */
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

/** The media id of the project's only asset. */
async function onlyMedia(page: Page): Promise<string> {
  const out = await editorCall<{ media: { id: string }[] }>(page, "media.list", {});
  const id = out.media[0]?.id;
  if (id === undefined) throw new Error("no media");
  return id;
}

/**
 * The agent adds the interview at 2 s on Video 1, then trims it to 4 s; after each step the UI
 * shows it (the timeline, and the inspector of the clip the person clicked). Returns the clip.
 */
async function agentEdits(app: ElectronApplication, page: Page, home: string): Promise<string> {
  const seq = await sequence(page);
  const media = await onlyMedia(page);
  const inserted = await agent(app, home, {
    cmd: "timeline.insert",
    args: { sequence: seq.id, track: seq.tracks[0]?.id, media, at: sec(2) },
  });
  expect(inserted).toBeTruthy();
  await expect
    .poll(() => layout(page))
    .toEqual([
      { track: "V1", clips: [[2, 10]] },
      { track: "A1", clips: [[2, 10]] },
    ]);
  await editorCaughtUp(page);
  await expect(page.getByText("Your timeline is empty")).toBeHidden();
  const id = (await sequence(page)).tracks[0]?.items[0]?.id ?? "";
  // It is on the timeline where the person can click it.
  await clickClip(page, id);
  await expect(inspector(page).start).toHaveValue("00:00:02:00");

  await agent(app, home, { cmd: "clip.trim", args: { item: id, edge: "end", to: sec(6) } });
  await expect(inspector(page).duration).toHaveValue("00:00:04:00");
  await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[2, 4]]);
  return id;
}

test.describe("F15 an agent edits while the UI is open", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  test.skip(!hasCli, "the cutweave CLI is not built (cargo build -p cutweave-cli)");

  test("CLI edits show up live in the UI", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, home, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Agent", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await agentEdits(app, page, home);
      await snapshot(page, info, "agent edits");
    } finally {
      await close();
      work.remove();
    }
  });

  test("an agent opening another project switches the UI to it", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, home, close } = await launchFlowApp(work);
    try {
      // A saved project with one clip of media, then a second, empty one in the window.
      await createProject(app, page, "Saved by hand", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4"), work.file("logo.png")]);
      await shortcut(app, "Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      await createProject(app, page, "Second", work.dir);
      await expect(mediaCards(page)).toHaveCount(0);

      // The agent opens the first one: the window follows, title, bin and inspector.
      await agent(app, home, {
        cmd: "project.open",
        args: { path: path.join(work.dir, "Saved by hand") },
      });
      await expect(page).toHaveTitle("Saved by hand — Cutweave");
      await expect(mediaCards(page)).toHaveCount(2);
      await editorCaughtUp(page);
      await expect(page.getByTestId("project-resolution")).toBeVisible();
      await expect(dirtyDot(page)).toBeHidden();
      await snapshot(page, info, "agent opened a project");
    } finally {
      await close();
      work.remove();
    }
  });

  test("Ctrl+Z in the UI undoes the agent's change", async () => {
    // Open question found by this flow (WP-1O.05), for the product owner: the flow catalog says
    // Ctrl+Z in the UI undoes the agent's edit, but history is per actor (arch §5.2) and the
    // editor's undo sends `edit.undo` without an actor, so it undoes the person's own last change.
    // Here that is the import, which the agent's clip now uses: the engine refuses it and the
    // person gets a raw conflict message ("Nothing was undone … history_target_modified …"),
    // while the Undo button reads "Undo Import media". Needs a decision: the UI's undo takes
    // anyone's latest change (`actor: "*"`), or a separate "undo the agent's change" action.
    // Owner: packages/editor (WP-1M.07) with arch §5.2.
    test.fixme(true, "the UI's Ctrl+Z only undoes the person's own changes (see the note)");
    const work = workspace();
    const { app, page, home, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Agent undo", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await agentEdits(app, page, home);
      // Ctrl+Z in the UI: the agent's trim comes off, then its insert.
      await expect(toolbar(page).getByRole("button", { name: /^Undo/ })).toBeEnabled();
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[2, 10]]);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([]);
    } finally {
      await close();
      work.remove();
    }
  });
});
