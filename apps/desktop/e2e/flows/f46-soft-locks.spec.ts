// SPDX-License-Identifier: GPL-3.0-or-later
// F46 Agents and the person on the same clip (docs/user-flows.md, WP-2A.12): while the person
// drags a clip, an agent (the real `cutweave exec`, `agent:cli`) tries to move it and is refused
// with `conflict` / `item_in_gesture` naming the person and when to retry; the clip does not move
// under the person's hand. Once the drag ends, the agent's retry moves it, and the Undo button
// names the agent. Then a copilot (a JSON-RPC client on the same agent token: a transaction is
// owned by its connection, and `cutweave exec` closes its connection after each call) holds the
// clip in an open proposal: the clip shows an "In use by cli" mark, the person's drag of it is
// refused with an agent-blue toast that names the agent and the machine code, and after the
// proposal is accepted the mark is gone and the person's drag works again.
import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import path from "node:path";
import { promisify } from "node:util";
import type { ElectronApplication, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  cliBin,
  clickClip,
  clipAt,
  createProject,
  dropCard,
  editorCaughtUp,
  hasCli,
  hasRealEngine,
  importWithButton,
  laneY,
  launchFlowApp,
  layout,
  sequence,
  snapshot,
  timelineGeometry,
  toast,
  toolbar,
  workspace,
  xAt,
} from "./helpers.ts";

const run = promisify(execFile);

async function enginePid(app: ElectronApplication): Promise<number> {
  const pid = await app.evaluate(() => globalThis.__cutweaveTest?.enginePid() ?? null);
  if (pid === null) throw new Error("no engine is running");
  return pid;
}

/** The app engine's run-file (in the app's HOME): its port and agent tokens. */
function runDir(home: string): string {
  return path.join(home, ".cutweave", "run");
}

/**
 * `cutweave exec <step>` attached to the app's engine, as an agent runs it: the exit code and
 * the step's output (exit 0) or the error the CLI printed (exit 1).
 */
async function agent(
  app: ElectronApplication,
  home: string,
  step: { cmd: string; args: Record<string, unknown> },
): Promise<{ exit: number; output: Record<string, unknown>; error: Record<string, unknown> }> {
  const pid = await enginePid(app);
  // Dev stub media in the CLI's own environment would make it spawn an engine of its own.
  const { CUTWEAVE_DEV_STUB_MEDIA: _stub, ...inherited } = process.env;
  const env = { ...inherited, HOME: home, USERPROFILE: home };
  const args = ["exec", "--run-dir", runDir(home), "--pid", String(pid), JSON.stringify(step)];
  const lastJson = (text: string | undefined) =>
    JSON.parse(
      (text ?? "")
        .split(/\r?\n/)
        .filter((l) => l.startsWith("{"))
        .pop() ?? "{}",
    ) as Record<string, unknown>;
  try {
    const { stdout } = await run(cliBin, args, { env, timeout: 30_000 });
    return { exit: 0, output: lastJson(stdout), error: {} };
  } catch (e) {
    const failed = e as { code?: number; stderr?: string };
    return { exit: failed.code ?? -1, output: {}, error: lastJson(failed.stderr) };
  }
}

/**
 * A copilot: a JSON-RPC WebSocket client of the app's engine with the run-file's `cli` agent
 * token (rpc.md "Credential"), so it edits as `agent:cli` and keeps its proposal open between
 * calls. Every wait is bounded.
 */
class Copilot {
  readonly #ws: WebSocket;
  #next = 1;
  readonly #pending = new Map<number, (message: Record<string, unknown>) => void>();

  private constructor(ws: WebSocket) {
    this.#ws = ws;
    ws.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data)) as Record<string, unknown>;
      const id = typeof message.id === "number" ? message.id : null;
      if (id !== null) this.#pending.get(id)?.(message);
    });
  }

  static async connect(app: ElectronApplication, home: string): Promise<Copilot> {
    const runFile = JSON.parse(
      readFileSync(path.join(runDir(home), `${await enginePid(app)}.json`), "utf8"),
    ) as { port: number; agent_tokens: { label: string; token: string }[] };
    const token = runFile.agent_tokens.find((t) => t.label === "cli")?.token;
    if (token === undefined) throw new Error("the run-file has no `cli` agent token");
    const ws = new WebSocket(`ws://127.0.0.1:${runFile.port}/rpc`, [`cutweave.token.${token}`]);
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error("the copilot did not connect in 10 s")),
        10_000,
      );
      ws.addEventListener("open", () => {
        clearTimeout(timer);
        resolve();
      });
      ws.addEventListener("error", () => {
        clearTimeout(timer);
        reject(new Error("the copilot could not connect"));
      });
    });
    return new Copilot(ws);
  }

  /** One call; resolves with its result, rejects with the engine's error. */
  call(
    method: string,
    params: Record<string, unknown>,
    tx?: string,
  ): Promise<Record<string, unknown>> {
    const id = this.#next++;
    const meta = { request_id: randomUUID(), ...(tx !== undefined && { tx }) };
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${method}: no answer in 10 s`)), 10_000);
      this.#pending.set(id, (message) => {
        clearTimeout(timer);
        this.#pending.delete(id);
        if (message.error !== undefined) reject(new Error(JSON.stringify(message.error)));
        else resolve(message.result as Record<string, unknown>);
      });
      this.#ws.send(
        JSON.stringify({ jsonrpc: "2.0", id, method, params: { ...params, _meta: meta } }),
      );
    });
  }

  close(): void {
    this.#ws.close();
  }
}

const clips = async (page: Page) => (await layout(page))[0]?.clips ?? [];

test.describe("F46 soft locks", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  test.skip(!hasCli, "the cutweave CLI is not built (cargo build -p cutweave-cli)");

  test("an agent never moves the clip the person is dragging; the person sees who holds a clip", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, home, close } = await launchFlowApp(work);
    let copilot: Copilot | undefined;
    try {
      await createProject(app, page, "Locks", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(() => clips(page)).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      const id = (await sequence(page)).tracks[0]?.items[0]?.id ?? "";
      // Zoom out so the clip stays in view wherever it goes.
      await clickClip(page, id);
      for (let i = 0; i < 3; i += 1) await page.keyboard.press("-");
      await expect.poll(async () => (await timelineGeometry(page)).pxPerSec).toBeCloseTo(51.2, 6);
      let g = await timelineGeometry(page);
      const px = (s: number) => s * g.pxPerSec;
      const v1 = laneY(g, "V1");

      // The person grabs the clip and drags it 1 s right, and keeps holding it.
      const grab = { x: xAt(g, clipAt(g, "V1", 1).start) + px(3), y: v1 };
      await page.mouse.move(grab.x, grab.y);
      await page.mouse.down();
      await page.mouse.move(grab.x + 4, grab.y, { steps: 2 });
      await page.mouse.move(grab.x + px(1), grab.y, { steps: 12 });
      await expect.poll(() => clips(page)).toEqual([[2, 10]]);

      // An agent tries to move it: refused, with who holds it and when to retry.
      const refused = await agent(app, home, {
        cmd: "item.move",
        args: { item: id, start: "3s" },
      });
      expect(refused.exit).toBe(1);
      expect(refused.error).toMatchObject({
        code: "conflict",
        reason: "item_in_gesture",
        holder: "user",
      });
      expect(refused.error.retry_after_ms).toBeGreaterThan(0);
      expect(refused.error.retry_after_ms).toBeLessThanOrEqual(30_000);
      expect(String(refused.error.message)).toContain("Move clip");
      // Nothing moved under the person's hand.
      expect(await clips(page)).toEqual([[2, 10]]);
      await snapshot(page, info, "dragging while the agent is refused");

      // The person lets go: the drag commits; the agent's retry now moves the clip.
      await page.mouse.up();
      await expect.poll(() => clips(page)).toEqual([[2, 10]]);
      await editorCaughtUp(page);
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });
      await expect(undo).toHaveAccessibleName("Undo Move clip");
      const retried = await agent(app, home, {
        cmd: "item.move",
        args: { item: id, start: "3s" },
      });
      expect(retried.exit).toBe(0);
      await expect.poll(() => clips(page)).toEqual([[3, 10]]);
      await expect(undo).toHaveAccessibleName("Undo Move clip (cli)");

      // A copilot holds the clip in an open proposal: the clip shows who has it.
      copilot = await Copilot.connect(app, home);
      const opened = await copilot.call("edit.begin_tx", { kind: "proposal", label: "Tidy up" });
      const tx = String(opened.tx);
      await copilot.call("item.move", { item: id, start: "4s" }, tx);
      await expect.poll(() => clips(page)).toEqual([[4, 10]]);
      // The clip and its linked sound (they moved together) each show the mark.
      const marks = page.getByRole("img", { name: "In use by cli" });
      await expect(marks).toHaveCount(2);
      const mark = page.locator(`[data-item="${id}"]`).getByRole("img", { name: "In use by cli" });
      await expect(mark).toBeVisible();
      await mark.hover();
      await expect(page.getByRole("tooltip", { name: "In use by cli" })).toBeVisible();
      await snapshot(page, info, "in use by the agent");

      // The person drags it anyway: refused, an agent-blue toast says who and the code.
      g = await timelineGeometry(page);
      const held = { x: xAt(g, clipAt(g, "V1", 4).start) + px(3), y: v1 };
      await page.mouse.move(held.x, held.y);
      await page.mouse.down();
      await page.mouse.move(held.x + 4, held.y, { steps: 2 });
      await page.mouse.move(held.x + px(2), held.y, { steps: 12 });
      await page.mouse.up();
      const refusal = toast(page, "Clip not moved");
      await expect(refusal).toBeVisible();
      await expect(refusal).toContainText(
        "In use by cli. Try again when they finish. (item_in_gesture)",
      );
      await expect(refusal).toHaveAttribute("data-tone", "agent");
      await expect.poll(() => clips(page)).toEqual([[4, 10]]);
      await snapshot(page, info, "refused toast naming the agent");

      // The proposal is accepted: the mark goes, the step is the agent's, the clip is free.
      await copilot.call("edit.commit_tx", { tx });
      await expect(marks).toHaveCount(0);
      await expect(undo).toHaveAccessibleName("Undo Tidy up (cli)");
      g = await timelineGeometry(page);
      const free = { x: xAt(g, clipAt(g, "V1", 4).start) + px(3), y: v1 };
      await page.mouse.move(free.x, free.y);
      await page.mouse.down();
      await page.mouse.move(free.x - 4, free.y, { steps: 2 });
      await page.mouse.move(free.x - px(1), free.y, { steps: 12 });
      await page.mouse.up();
      await expect.poll(() => clips(page)).toEqual([[3, 10]]);
      await expect(undo).toHaveAccessibleName("Undo Move clip");
    } finally {
      copilot?.close();
      await close();
      work.remove();
    }
  });
});
