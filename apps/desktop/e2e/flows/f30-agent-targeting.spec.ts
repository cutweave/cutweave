// SPDX-License-Identifier: GPL-3.0-or-later
// F30 An agent finds the window you are looking at (docs/user-flows.md, WP-1J.04): the app is
// open on a project and a script has left a headless engine running. The `cutweave` CLI, given
// no pid and no project, attaches to the app's engine, so its edit shows in the window. The
// engines are listed with their projects.
import { type ChildProcess, execFile, spawn } from "node:child_process";
import path from "node:path";
import { createInterface } from "node:readline";
import { promisify } from "node:util";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  cliBin,
  createProject,
  editorCaughtUp,
  hasCli,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  layout,
  sec,
  sequence,
  snapshot,
  workspace,
} from "./helpers.ts";

const run = promisify(execFile);

interface EngineEntry {
  pid: number;
  mode: "gui" | "headless";
  project: { name: string; path: string } | null;
  focused_at?: number;
}

/** `cutweave <args>` as an agent would run it: HOME is the app's, so the run-files are shared. */
async function cutweave(home: string, args: string[]): Promise<unknown> {
  // Dev stub media in the CLI's own environment would make it spawn an engine of its own.
  const { CUTWEAVE_DEV_STUB_MEDIA: _stub, ...inherited } = process.env;
  const env = { ...inherited, HOME: home, USERPROFILE: home };
  const { stdout } = await run(cliBin, args, { env, timeout: 30_000 });
  return JSON.parse(stdout) as unknown;
}

/** A headless engine a script left running, in the same run-file folder as the app's. */
async function startHeadlessEngine(home: string): Promise<{ child: ChildProcess; pid: number }> {
  const child = spawn(
    cliBin,
    [
      "engine",
      "--headless",
      "--port",
      "0",
      "--no-log",
      "--run-dir",
      path.join(home, ".cutweave", "run"),
    ],
    { stdio: ["ignore", "pipe", "ignore"], env: { ...process.env, HOME: home, USERPROFILE: home } },
  );
  const lines = createInterface({ input: child.stdout ?? process.stdin });
  const pid = await new Promise<number>((resolve, reject) => {
    child.once("exit", () => reject(new Error("the headless engine exited before it was ready")));
    lines.once("line", (line) => {
      const ready = JSON.parse(line) as { event?: string; pid?: number };
      if (ready.event === "ready" && typeof ready.pid === "number") resolve(ready.pid);
      else reject(new Error(`unexpected engine line: ${line}`));
    });
  });
  lines.close();
  return { child, pid };
}

async function stopEngine(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
  child.kill();
  await exited;
}

async function onlyMedia(page: Page): Promise<string> {
  const out = await editorCall<{ media: { id: string }[] }>(page, "media.list", {});
  const id = out.media[0]?.id;
  if (id === undefined) throw new Error("no media");
  return id;
}

test.describe("F30 an agent finds the window you are looking at", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  test.skip(!hasCli, "the cutweave CLI is not built (cargo build -p cutweave-cli)");

  test("with no pid and no project the agent edits the app's project", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, home, close } = await launchFlowApp(work);
    let headless: { child: ChildProcess; pid: number } | null = null;
    try {
      await createProject(app, page, "Front", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      headless = await startHeadlessEngine(home);
      const runDir = path.join(home, ".cutweave", "run");

      // Both engines are listed, the app's first, with the project it shows. The window has
      // the focus, so its engine says when.
      await page.bringToFront();
      let engines: EngineEntry[] = [];
      await expect
        .poll(async () => {
          engines = (await cutweave(home, [
            "engines",
            "--json",
            "--run-dir",
            runDir,
          ])) as EngineEntry[];
          const gui = engines[0];
          return (
            gui?.mode === "gui" && gui.project?.name === "Front" && gui.focused_at !== undefined
          );
        })
        .toBe(true);
      expect(engines).toHaveLength(2);
      expect(engines[1]).toMatchObject({ pid: headless.pid, mode: "headless", project: null });

      // No --pid, no --project: the agent's edit lands in the window's project, not the script's.
      const seq = await sequence(page);
      const media = await onlyMedia(page);
      const step = {
        cmd: "timeline.insert",
        args: { sequence: seq.id, track: seq.tracks[0]?.id, media, at: sec(2) },
      };
      await cutweave(home, ["exec", "--run-dir", runDir, JSON.stringify(step)]);
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[2, 10]] },
          { track: "A1", clips: [[2, 10]] },
        ]);
      await editorCaughtUp(page);
      await expect(page.getByText("Your timeline is empty")).toBeHidden();

      // Naming the project reaches the same engine.
      const named = (await cutweave(home, [
        "query",
        "project.info",
        "--run-dir",
        runDir,
        "--project",
        "front",
      ])) as { output: { name: string } };
      expect(named.output.name).toBe("Front");
      await snapshot(page, info, "agent without flags");
    } finally {
      if (headless !== null) await stopEngine(headless.child);
      await close();
      work.remove();
    }
  });
});
