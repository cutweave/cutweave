// SPDX-License-Identifier: GPL-3.0-or-later
// Launch-protocol helpers, engine resolution, ports, and the supervisor against the fake engine
// (test-support/fake-engine.ts) as a real child process.

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { type AddressInfo, createServer } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import type { EngineStatus } from "../../shared/api.ts";
import { devEngineArgs, preferenceEngineArgs, resolveEngineLaunch } from "./launch.ts";
import { isPortFree, pickFreePort } from "./ports.ts";
import { parseEngineLine, parseReadyLine, readCrashMarker } from "./protocol.ts";
import { connectEngineRpc, EngineRpcError, uuidv7 } from "./rpc-client.ts";
import { EngineSupervisor } from "./supervisor.ts";

const fake = fileURLToPath(new URL("../../../test-support/fake-engine.ts", import.meta.url));
const launch = { command: process.execPath, args: [fake, "engine"] };
const quiet = { log: () => {} };

describe("launch protocol lines", () => {
  it("accepts only the readiness event as ready", () => {
    expect(parseReadyLine('{"event":"ready","port":9,"pid":3,"version":"x"}')).toEqual({
      port: 9,
      pid: 3,
      version: "x",
    });
    expect(parseReadyLine('{"event":"log","port":9,"pid":3}')).toBeNull();
    expect(parseReadyLine("garbage")).toBeNull();
  });

  it("parses the starting line of --progress, which is not a ready line", () => {
    const line = '{"event":"starting","pid":3,"version":"x","elapsed_ms":2000}';
    expect(parseEngineLine(line)).toEqual({
      event: "starting",
      starting: { pid: 3, elapsedMs: 2000 },
    });
    expect(parseReadyLine(line)).toBeNull();
    expect(parseEngineLine('{"event":"starting","version":"x"}')).toBeNull();
  });

  it("parses the error line, with the lock holder", () => {
    expect(
      parseEngineLine(
        '{"event":"error","code":"project_locked","exit_code":10,"message":"held","holder":{"pid":4,"host":"H","started":1}}',
      ),
    ).toEqual({
      event: "error",
      error: {
        code: "project_locked",
        exitCode: 10,
        message: "held",
        holder: { pid: 4, host: "H", started: 1 },
      },
    });
    expect(parseEngineLine('{"event":"error","code":"usage"}')).toBeNull();
    expect(parseEngineLine("[1]")).toBeNull();
  });

  it("reads a crash marker left by a dead engine", () => {
    const dir = mkdtempSync(path.join(tmpdir(), "cw-marker-"));
    try {
      expect(readCrashMarker(dir, 7)).toBeNull();
      writeFileSync(
        path.join(dir, "crash-7.json"),
        JSON.stringify({ event: "crash", pid: 7, message: "boom", location: "a.rs:2" }),
      );
      expect(readCrashMarker(dir, 7)).toEqual({ message: "boom", location: "a.rs:2" });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("resolveEngineLaunch", () => {
  const base = {
    env: {},
    isPackaged: false,
    resourcesPath: "/res",
    repoRoot: "/repo",
    platform: "linux" as const,
  };

  it("uses the fake or CUTWEAVE_ENGINE_BIN in dev, never when packaged", () => {
    expect(
      resolveEngineLaunch({ ...base, env: { CUTWEAVE_ENGINE_FAKE: "/f.ts" }, exists: () => false }),
    ).toEqual({ command: "node", args: ["/f.ts", "engine"] });
    expect(
      resolveEngineLaunch({ ...base, env: { CUTWEAVE_ENGINE_BIN: "/bin/e" }, exists: () => false }),
    ).toEqual({ command: "/bin/e", args: ["engine"] });
    expect(
      resolveEngineLaunch({
        ...base,
        isPackaged: true,
        env: { CUTWEAVE_ENGINE_FAKE: "/f.ts", CUTWEAVE_ENGINE_BIN: "/evil" },
        exists: () => false,
      }),
    ).toBeNull();
  });

  it("finds the bundled engine, then the dev builds", () => {
    expect(resolveEngineLaunch({ ...base, isPackaged: true, exists: () => true })?.command).toBe(
      path.join("/res", "engine", "cutweave"),
    );
    expect(
      resolveEngineLaunch({ ...base, platform: "win32", isPackaged: true, exists: () => true })
        ?.command,
    ).toMatch(/cutweave\.exe$/);
    expect(
      resolveEngineLaunch({ ...base, exists: (f) => f.endsWith(path.join("debug", "cutweave")) }),
    ).toEqual({ command: path.join("/repo", "target", "debug", "cutweave"), args: ["engine"] });
    // Until `cutweave engine` exists (WP-1K.01): the launch-protocol binary.
    expect(
      resolveEngineLaunch({ ...base, exists: (f) => f.endsWith("cutweave-engine-serve") }),
    ).toEqual({
      command: path.join("/repo", "target", "debug", "cutweave-engine-serve"),
      args: ["engine"],
    });
  });
});

describe("devEngineArgs", () => {
  it("turns CUTWEAVE_ENGINE_ALLOW_ROOTS into --allow-root flags in dev", () => {
    const env = { CUTWEAVE_ENGINE_ALLOW_ROOTS: " /a/media ;; /b " };
    expect(devEngineArgs(env, false, ";")).toEqual([
      "--allow-root",
      "/a/media",
      "--allow-root",
      "/b",
    ]);
  });

  it("adds nothing when unset or when packaged", () => {
    expect(devEngineArgs({}, false, ";")).toEqual([]);
    expect(devEngineArgs({ CUTWEAVE_ENGINE_ALLOW_ROOTS: "/a" }, true, ";")).toEqual([]);
  });
});

describe("preferenceEngineArgs", () => {
  it("passes a chosen cache folder as --cache-dir", () => {
    expect(preferenceEngineArgs({ cacheFolder: "D:\\Cutweave cache" })).toEqual([
      "--cache-dir",
      "D:\\Cutweave cache",
    ]);
  });

  it("leaves the engine its default when no folder is chosen", () => {
    expect(preferenceEngineArgs({ cacheFolder: "" })).toEqual([]);
  });
});

describe("ports", () => {
  it("tells a free port from a taken one", async () => {
    const blocker = createServer();
    await new Promise<void>((resolve) => blocker.listen(0, "127.0.0.1", resolve));
    const taken = (blocker.address() as AddressInfo).port;
    try {
      expect(await isPortFree(taken)).toBe(false);
    } finally {
      await new Promise((resolve) => blocker.close(resolve));
    }
    expect(await isPortFree(taken)).toBe(true);
    expect(await pickFreePort()).toBeGreaterThan(0);
  });
});

describe("uuidv7", () => {
  it("is a time-ordered v7 UUID", () => {
    const a = uuidv7(1_000);
    const b = uuidv7(2_000);
    expect(a).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(a.slice(0, 13) < b.slice(0, 13)).toBe(true);
    expect(a.slice(0, 13)).toBe("00000000-03e8");
  });
});

describe("EngineSupervisor with the fake engine", () => {
  let sup: EngineSupervisor | null = null;
  afterEach(async () => {
    await sup?.stop();
    sup = null;
  });

  it("starts, reports ready on the chosen port, and stops cleanly", async () => {
    const port = await pickFreePort();
    sup = new EngineSupervisor({ launch, port, ...quiet });
    const states: string[] = [];
    sup.onStatus((s) => states.push(s.state));
    expect(await sup.start()).toMatchObject({ state: "ready", port, generation: 1 });
    expect((await fetch(`http://127.0.0.1:${port}/`)).status).toBe(200);
    await sup.stop();
    expect(states).toEqual(["starting", "ready", "stopped"]);
  });

  it("requires the UI token on /rpc, over HTTP and WebSocket", async () => {
    const port = await pickFreePort();
    sup = new EngineSupervisor({ launch, port, ...quiet });
    await sup.start();
    const token = sup.credentials?.token ?? "";
    expect(token).toMatch(/^[0-9a-f]{64}$/);
    expect((await fetch(`http://127.0.0.1:${port}/rpc`)).status).toBe(401);
    const authed = await fetch(`http://127.0.0.1:${port}/rpc`, {
      headers: { authorization: `Bearer ${token}` },
    });
    expect(authed.status).toBe(200);
    await expect(connectEngineRpc(port, "f".repeat(64))).rejects.toThrow(/refused|closed/);
    const rpc = await connectEngineRpc(port, token);
    expect(await rpc.call("rpc.hello", { client: "t", version: "0" })).toMatchObject({
      actor: "user",
    });
    await expect(rpc.call("no.such")).rejects.toBeInstanceOf(EngineRpcError);
    rpc.close();
  });

  it("fails when the engine exits with a fatal code, keeping the error line", async () => {
    sup = new EngineSupervisor({
      launch,
      port: await pickFreePort(),
      env: { FAKE_ENGINE_EXIT: "10" },
      ...quiet,
    });
    const status = await sup.start();
    expect(status).toMatchObject({
      state: "failed",
      lastError: { code: "project_locked", exitCode: 10, holder: { pid: 4242 } },
    });
  });

  it("restarts after a crash and recovers the project with a fresh token", async () => {
    sup = new EngineSupervisor({
      launch,
      port: await pickFreePort(),
      env: { FAKE_ENGINE_RECOVERY: "1" },
      ...quiet,
    });
    const seen: EngineStatus[] = [];
    sup.onStatus((s) => seen.push(s));
    await sup.start();
    const first = sup.credentials;
    // The editor opens a project; main learns it from the project events.
    const editor = await connectEngineRpc(first?.port ?? 0, first?.token ?? "");
    await editor.call("project.open", { path: "/fake/film.cutweave" });
    editor.close();
    await expect.poll(() => sup?.project).toBe("/fake/film.cutweave");
    expect(sup.kill()).toBeGreaterThan(0);
    await expect
      .poll(() => sup?.status.generation === 2 && sup.status.state, { timeout: 10_000 })
      .toBe("ready");
    expect(seen.map((s) => s.state)).toEqual([
      "starting",
      "ready",
      "crashed",
      "restarting",
      "ready",
    ]);
    expect(sup.status.recovery).toEqual({
      project: "/fake/film.cutweave",
      applied: 3,
      damaged: false,
    });
    expect(sup.credentials?.port).toBe(first?.port);
    expect(sup.credentials?.token).not.toBe(first?.token);
    // The old token is dead with the old engine.
    expect(
      (
        await fetch(`http://127.0.0.1:${first?.port}/rpc`, {
          headers: { authorization: `Bearer ${first?.token}` },
        })
      ).status,
    ).toBe(401);
  });

  it("times out when no readiness line arrives", async () => {
    sup = new EngineSupervisor({
      launch,
      port: await pickFreePort(),
      env: { FAKE_ENGINE_MODE: "no-ready" },
      startSilenceMs: 300,
      backoffMs: [],
      ...quiet,
    });
    expect(await sup.start()).toMatchObject({
      state: "failed",
      lastError: { code: "ready_timeout" },
    });
  });

  it("reports a spawn error as failed", async () => {
    sup = new EngineSupervisor({
      launch: { command: "definitely-not-a-binary-xyz", args: [] },
      port: 1,
      ...quiet,
    });
    expect(await sup.start()).toMatchObject({
      state: "failed",
      lastError: { code: "spawn_failed" },
    });
  });

  it("re-picks the port when the engine cannot bind it (exit 12)", async () => {
    const blocker = createServer();
    await new Promise<void>((resolve) => blocker.listen(0, "127.0.0.1", resolve));
    const taken = (blocker.address() as AddressInfo).port;
    const changes: number[][] = [];
    try {
      sup = new EngineSupervisor({
        launch,
        port: taken,
        onPortChange: (port, previous) => changes.push([port, previous]),
        ...quiet,
      });
      const status = await sup.start();
      expect(status.state).toBe("ready");
      expect(status.port).not.toBe(taken);
      expect(changes).toEqual([[status.port, taken]]);
    } finally {
      blocker.close();
    }
  });
});
