// SPDX-License-Identifier: GPL-3.0-or-later
import { type AddressInfo, createServer } from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { EngineStatus } from "../../shared/api.ts";
import { pickFreePort, resolveEngineLaunch, startWithPortRetry } from "./launch.ts";
import {
  EngineStartError,
  EngineSupervisor,
  EXIT_PORT_IN_USE,
  parseReadyLine,
} from "./supervisor.ts";

const fake = fileURLToPath(new URL("../../../test-support/fake-engine.ts", import.meta.url));
const launch = { command: process.execPath, args: [fake, "engine"] };

describe("parseReadyLine", () => {
  it("accepts only the readiness event", () => {
    expect(parseReadyLine('{"event":"ready","port":9,"pid":3,"version":"x"}')).toEqual({
      port: 9,
      pid: 3,
      version: "x",
    });
    expect(parseReadyLine('{"event":"log","port":9,"pid":3}')).toBeNull();
    expect(parseReadyLine("garbage")).toBeNull();
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

  it("uses the fake in dev, ignores overrides when packaged, finds the bundled engine", () => {
    expect(
      resolveEngineLaunch({ ...base, env: { CUTWEAVE_ENGINE_FAKE: "/f.ts" }, exists: () => false }),
    ).toEqual({ command: "node", args: ["/f.ts", "engine"] });
    expect(
      resolveEngineLaunch({
        ...base,
        isPackaged: true,
        env: { CUTWEAVE_ENGINE_FAKE: "/f.ts", CUTWEAVE_ENGINE_BIN: "/evil" },
        exists: () => false,
      }),
    ).toBeNull();
    expect(resolveEngineLaunch({ ...base, isPackaged: true, exists: () => true })?.command).toBe(
      path.join("/res", "engine", "cutweave"),
    );
    expect(
      resolveEngineLaunch({ ...base, exists: (f) => f.endsWith(path.join("debug", "cutweave")) })
        ?.command,
    ).toBe(path.join("/repo", "target", "debug", "cutweave"));
    expect(
      resolveEngineLaunch({ ...base, platform: "win32", isPackaged: true, exists: () => true })
        ?.command,
    ).toMatch(/cutweave\.exe$/);
  });
});

describe("startWithPortRetry", () => {
  const taken = () => Promise.reject(new EngineStartError("port taken", EXIT_PORT_IN_USE));

  it("re-picks the port on exit 12 and reports the change", async () => {
    const start = vi.fn((port: number) => (port === 1000 ? taken() : Promise.resolve(port)));
    const onPortChange = vi.fn();
    const ports = [1000, 2000];
    const result = await startWithPortRetry({
      initialPort: 1000,
      start,
      pickPort: async () => ports.shift() ?? 3000,
      onPortChange,
    });
    expect(result).toBe(2000);
    expect(start.mock.calls.map(([p]) => p)).toEqual([1000, 2000]);
    expect(onPortChange).toHaveBeenCalledExactlyOnceWith(2000, 1000);
  });

  it("gives up after maxAttempts and rethrows other failures unchanged", async () => {
    let next = 1;
    const onPortChange = vi.fn();
    await expect(
      startWithPortRetry({
        initialPort: 1,
        maxAttempts: 3,
        start: taken,
        pickPort: async () => ++next,
        onPortChange,
      }),
    ).rejects.toThrow(/port taken/);
    expect(onPortChange).toHaveBeenCalledTimes(2);

    const other = new EngineStartError("crashed", 70);
    const noChange = vi.fn();
    await expect(
      startWithPortRetry({
        initialPort: 1,
        start: () => Promise.reject(other),
        pickPort: async () => 2,
        onPortChange: noChange,
      }),
    ).rejects.toBe(other);
    expect(noChange).not.toHaveBeenCalled();
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
    sup = new EngineSupervisor({ launch, port, uiToken: "a".repeat(64) });
    const states: string[] = [];
    sup.onStatus((s) => states.push(s.state));
    const ready = await sup.start();
    expect(ready.port).toBe(port);
    expect((await fetch(`http://127.0.0.1:${port}/`)).status).toBe(200);
    await sup.stop();
    expect(states).toEqual(["starting", "ready", "stopped"]);
  });

  it("fails when the engine exits before ready", async () => {
    sup = new EngineSupervisor({
      launch,
      port: await pickFreePort(),
      uiToken: "t",
      env: { FAKE_ENGINE_MODE: "exit-early" },
    });
    await expect(sup.start()).rejects.toThrow(/exited before ready/);
    expect(sup.status.state).toBe("failed");
  });

  it("times out when no readiness line arrives", async () => {
    sup = new EngineSupervisor({
      launch,
      port: await pickFreePort(),
      uiToken: "t",
      env: { FAKE_ENGINE_MODE: "no-ready" },
      readyTimeoutMs: 400,
    });
    await expect(sup.start()).rejects.toThrow(/in time/);
  });

  it("reports a crash after ready", async () => {
    sup = new EngineSupervisor({
      launch,
      port: await pickFreePort(),
      uiToken: "t",
      env: { FAKE_ENGINE_MODE: "crash-after-ready" },
    });
    const seen: EngineStatus[] = [];
    sup.onStatus((s) => seen.push(s));
    await sup.start();
    await expect.poll(() => seen.at(-1)?.state, { timeout: 5000 }).toBe("crashed");
  });

  it("rejects with exit code 12 when the port is taken", async () => {
    const blocker = createServer();
    await new Promise<void>((resolve) => blocker.listen(0, "127.0.0.1", resolve));
    const taken = (blocker.address() as AddressInfo).port;
    try {
      sup = new EngineSupervisor({ launch, port: taken, uiToken: "t" });
      const err = await sup.start().catch((e: unknown) => e);
      expect(err).toBeInstanceOf(EngineStartError);
      expect((err as EngineStartError).exitCode).toBe(EXIT_PORT_IN_USE);
    } finally {
      blocker.close();
    }
  });

  it("requires the UI token on /rpc", async () => {
    const port = await pickFreePort();
    sup = new EngineSupervisor({ launch, port, uiToken: "b".repeat(64) });
    await sup.start();
    expect((await fetch(`http://127.0.0.1:${port}/rpc`)).status).toBe(401);
    const authed = await fetch(`http://127.0.0.1:${port}/rpc`, {
      headers: { authorization: `Bearer ${"b".repeat(64)}` },
    });
    expect(authed.status).toBe(200);
  });

  it("reports a spawn error as failed", async () => {
    sup = new EngineSupervisor({
      launch: { command: "definitely-not-a-binary-xyz", args: [] },
      port: 1,
      uiToken: "t",
    });
    await expect(sup.start()).rejects.toThrow(/failed to start/);
  });
});
