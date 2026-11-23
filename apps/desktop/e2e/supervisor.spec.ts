// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1L.03 main-process half: the engine supervisor inside the real app, with the fake engine
// (and the real one when `cutweave-engine-serve` is built). The renderer half (resend by
// request_id, resync, the notice, blocked editing) is e2e/resync.spec.ts.

import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import type { ElectronApplication, Page } from "@playwright/test";
import {
  engineStatus,
  expect,
  hasRealEngine,
  killEngine,
  launchApp,
  REAL_ENGINE_ENV,
  test,
} from "./fixtures.ts";

const waitForGeneration = (app: ElectronApplication, generation: number) =>
  expect
    .poll(
      async () => {
        const s = await engineStatus(app);
        return s?.generation === generation ? s.state : `gen ${s?.generation} ${s?.state}`;
      },
      { timeout: 20_000 },
    )
    .toBe("ready");

const connection = (page: Page) => page.evaluate(() => window.cutweave.engine.getConnection());

/**
 * The engine's auth answer to `/rpc` with a bearer token, asked from the test process (from the
 * page, an Authorization header would need a CORS preflight the fake engine does not answer).
 */
const rpcStatus = async (port: number, token: string) =>
  (await fetch(`http://127.0.0.1:${port}/rpc`, { headers: { authorization: `Bearer ${token}` } }))
    .status;

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function filesUnder(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...filesUnder(p));
    else if (entry.isFile()) out.push(p);
  }
  return out;
}

test("a killed engine is restarted on the same port with a new UI token", async ({ app, page }) => {
  await waitForGeneration(app, 1);
  await page.evaluate(() => {
    const seen: string[] = [];
    (window as unknown as { __engineStates: string[] }).__engineStates = seen;
    window.cutweave.engine.onStatus((s) => seen.push(s.state));
  });
  const before = await connection(page);
  if (before === null) throw new Error("no connection while ready");

  await killEngine(app);
  await waitForGeneration(app, 2);
  const after = await connection(page);
  expect(after?.port).toBe(before.port);
  expect(after?.token).not.toBe(before.token);
  // The renderer was told: crashed, restarting, ready (the token is never in a status event).
  await expect
    .poll(() =>
      page.evaluate(() => (window as unknown as { __engineStates: string[] }).__engineStates),
    )
    .toEqual(["crashed", "restarting", "ready"]);
  expect(JSON.stringify(await engineStatus(app))).not.toContain(after?.token ?? "?");
  // The new engine takes only the new token.
  expect(await rpcStatus(before.port, before.token)).toBe(401);
  expect(await rpcStatus(before.port, after?.token ?? "")).toBe(200);
});

test("a restart reopens the open project and recovers it without a prompt", async () => {
  const { app, page, close } = await launchApp({ FAKE_ENGINE_RECOVERY: "1" });
  try {
    await waitForGeneration(app, 1);
    // The editor opens a project over its own engine socket; main follows the project events.
    const opened = await page.evaluate(async () => {
      const c = await window.cutweave.engine.getConnection();
      if (c === null) return "no connection";
      const ws = new WebSocket(c.wsUrl, [`cutweave.token.${c.token}`]);
      return new Promise<string>((resolve) => {
        ws.onopen = () =>
          ws.send(
            JSON.stringify({
              jsonrpc: "2.0",
              id: 1,
              method: "project.open",
              params: { path: "/work/film.cutweave" },
            }),
          );
        ws.onmessage = (e) => {
          ws.close();
          resolve(String(e.data));
        };
        ws.onerror = () => resolve("socket error");
      });
    });
    expect(opened).toContain("/work/film.cutweave");

    await killEngine(app);
    await waitForGeneration(app, 2);
    expect((await engineStatus(app))?.recovery).toEqual({
      project: "/work/film.cutweave",
      applied: 3,
      damaged: false,
    });
  } finally {
    await close();
  }
});

test("a fourth crash within 60 s is failed; the dialog's Restart brings the engine back", async ({
  app,
}) => {
  await app.evaluate(() => globalThis.__cutweaveTest?.captureEngineDialog());
  await waitForGeneration(app, 1);
  for (let generation = 2; generation <= 4; generation++) {
    await killEngine(app);
    await waitForGeneration(app, generation);
  }
  await killEngine(app);
  await expect.poll(async () => (await engineStatus(app))?.state).toBe("failed");
  const dialogs = await app.evaluate(() => globalThis.__cutweaveTest?.engineDialogs() ?? []);
  expect(dialogs).toHaveLength(1);
  expect(dialogs[0]).toMatchObject({ state: "failed", lastError: { code: "crashed" } });
  expect(dialogs[0]?.message).toMatch(/4 times in 60 s/);

  expect(await app.evaluate(() => globalThis.__cutweaveTest?.answerEngineDialog("restart"))).toBe(
    true,
  );
  await waitForGeneration(app, 5);
});

test("quitting stops the engine and leaves no engine process or UI token behind", async () => {
  const { app, page, home, close } = await launchApp();
  try {
    await waitForGeneration(app, 1);
    const tokens = [(await connection(page))?.token ?? ""];
    await killEngine(app);
    await waitForGeneration(app, 2);
    tokens.push((await connection(page))?.token ?? "");
    const pid = await app.evaluate(() => globalThis.__cutweaveTest?.enginePid() ?? null);
    expect(pid).not.toBeNull();

    await app.close();
    await expect.poll(() => alive(pid ?? 0), { timeout: 10_000 }).toBe(false);
    // Nothing under the throwaway HOME (the Electron profile, logs, run-files) holds a token.
    for (const file of filesUnder(home)) {
      const bytes = readFileSync(file);
      for (const token of tokens) {
        expect(token).toMatch(/^[0-9a-f]{64}$/);
        expect(bytes.includes(token), file).toBe(false);
      }
    }
  } finally {
    await close();
  }
});

test("with the real engine: the editor's socket is accepted before and after a restart", async () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  const { app, page, close } = await launchApp(REAL_ENGINE_ENV);
  /** `rpc.hello` over the editor's own socket (Origin app://cutweave, token subprotocol). */
  const hello = () =>
    page.evaluate(async () => {
      const c = await window.cutweave.engine.getConnection();
      if (c === null) return "no connection";
      const ws = new WebSocket(c.wsUrl, [`cutweave.token.${c.token}`]);
      return new Promise<string>((resolve) => {
        ws.onopen = () =>
          ws.send(
            JSON.stringify({
              jsonrpc: "2.0",
              id: 1,
              method: "rpc.hello",
              params: { client: "e2e", version: "0" },
            }),
          );
        ws.onmessage = (e) => {
          ws.close();
          resolve(String(e.data));
        };
        ws.onerror = () => resolve("socket error");
      });
    });
  try {
    await waitForGeneration(app, 1);
    expect(await hello()).toContain('"actor":"user"');
    await killEngine(app);
    await waitForGeneration(app, 2);
    expect(await hello()).toContain('"actor":"user"');
  } finally {
    await close();
  }
});
