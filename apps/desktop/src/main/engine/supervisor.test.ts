// SPDX-License-Identifier: GPL-3.0-or-later
// EngineSupervisor state machine with fake timers, a fake child process and a fake control
// connection (WP-1L.03). The real-engine round trip is in supervisor.integration.test.ts.

import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineStatus } from "../../shared/api.ts";
import { engineAllowRoots } from "../agents.ts";
import type { EngineRpc, NotificationListener } from "./rpc-client.ts";
import { type ChildLike, EngineSupervisor, type SupervisorOptions } from "./supervisor.ts";

class FakeChild extends EventEmitter {
  readonly stdin = new PassThrough();
  readonly stdout = new PassThrough();
  readonly signals: (NodeJS.Signals | undefined)[] = [];
  stdinText = "";
  stdinEnded = false;
  constructor(
    readonly pid: number | undefined,
    readonly args: readonly string[],
    readonly env: NodeJS.ProcessEnv,
  ) {
    super();
    this.stdin.on("data", (d: Buffer) => {
      this.stdinText += d.toString("utf8");
    });
    this.stdin.on("finish", () => {
      this.stdinEnded = true;
    });
  }
  kill(signal?: NodeJS.Signals): boolean {
    this.signals.push(signal);
    return true;
  }
  arg(name: string): string | undefined {
    const i = this.args.indexOf(name);
    return i >= 0 ? this.args[i + 1] : undefined;
  }
  ready(port = Number(this.arg("--port"))): void {
    this.stdout.write(`${JSON.stringify({ event: "ready", port, pid: this.pid, version: "t" })}\n`);
  }
  errorLine(code: string, exitCode: number, extra: object = {}): void {
    const line = { event: "error", code, exit_code: exitCode, message: `${code}!`, ...extra };
    this.stdout.write(`${JSON.stringify(line)}\n`);
  }
  exit(code: number | null, signal: NodeJS.Signals | null = null): void {
    this.emit("exit", code, signal);
  }
}

/** The engine side the fake control connection reads: which project, whether recovery waits. */
interface EngineSide {
  project: string | null;
  recovery: boolean;
}

class FakeRpc implements EngineRpc {
  readonly calls: { method: string; params: Record<string, unknown> | undefined }[] = [];
  listener: NotificationListener | null = null;
  closed = false;
  constructor(readonly side: EngineSide) {}
  call(method: string, params?: Record<string, unknown>): Promise<unknown> {
    this.calls.push({ method, params });
    if (method === "rpc.hello") {
      return Promise.resolve({
        project: {
          path: this.side.project,
          recovery: this.side.recovery ? { records: 3 } : null,
        },
      });
    }
    if (method === "project.recover") {
      const applied = this.side.recovery ? 3 : 0;
      this.side.recovery = false;
      return Promise.resolve({ applied, damaged: false });
    }
    return Promise.resolve({});
  }
  onNotification(listener: NotificationListener): void {
    this.listener = listener;
  }
  #onClose: (() => void) | null = null;
  onClose(listener: () => void): void {
    this.#onClose = listener;
  }
  close(): void {
    this.closed = true;
  }
  /** The engine closed the socket (e.g. 4001 `resync_required`). */
  drop(): void {
    this.closed = true;
    this.#onClose?.();
  }
  methods(): string[] {
    return this.calls.map((c) => c.method);
  }
}

/** Let readline, stream and promise callbacks run (they use real ticks, timers are fake). */
async function flush(): Promise<void> {
  for (let i = 0; i < 10; i++) await new Promise<void>((r) => setImmediate(r));
}

function harness(overrides: Partial<SupervisorOptions> = {}) {
  const children: FakeChild[] = [];
  const rpcs: FakeRpc[] = [];
  const side: EngineSide = { project: null, recovery: false };
  const statuses: EngineStatus[] = [];
  const portChanges: [number, number][] = [];
  const busy = new Set<number>();
  let nextPort = 5000;
  let nextPid = 100;
  let tokenSeq = 0;
  const sup = new EngineSupervisor({
    launch: { command: "engine-bin", args: ["engine"] },
    port: 4000,
    extraArgs: ["--allow-origin", "app://cutweave"],
    log: () => {},
    spawn: (_command, args, options) => {
      const child = new FakeChild(nextPid++, args, options.env);
      children.push(child);
      return child as unknown as ChildLike;
    },
    generateToken: () => `${String(++tokenSeq).padStart(2, "0")}${"f".repeat(62)}`,
    isPortFree: async (port) => !busy.has(port),
    pickPort: async () => nextPort++,
    connectRpc: async () => {
      const rpc = new FakeRpc(side);
      rpcs.push(rpc);
      return rpc;
    },
    readCrashMarker: () => null,
    onPortChange: (port, previous) => portChanges.push([port, previous]),
    ...overrides,
  });
  sup.onStatus((s) => statuses.push(s));
  const last = () => {
    const child = children.at(-1);
    if (child === undefined) throw new Error("no engine spawned");
    return child;
  };
  /** Bring the newest engine to `ready` (ready line + control connection). */
  const ready = async () => {
    last().ready();
    await flush();
  };
  const states = () => statuses.map((s) => s.state);
  return { sup, children, rpcs, side, statuses, states, portChanges, busy, last, ready };
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
});
afterEach(() => {
  vi.useRealTimers();
});

describe("start and the handshake", () => {
  it("sends the token on stdin only and reports ready on the chosen port", async () => {
    const h = harness();
    const settled = h.sup.start();
    await flush();
    const child = h.last();
    const token = child.stdinText.trim();
    expect(token).toMatch(/^[0-9a-f]{64}$/);
    expect(child.stdinText).toBe(`${token}\n`);
    expect(child.args).toEqual([
      "engine",
      "--port",
      "4000",
      "--token-stdin",
      "--allow-origin",
      "app://cutweave",
    ]);
    // Never in argv or the environment.
    expect(child.args.join(" ")).not.toContain(token);
    expect(Object.values(child.env).join("\n")).not.toContain(token);
    expect(h.sup.credentials).toBeNull();

    await h.ready();
    expect(await settled).toMatchObject({ state: "ready", port: 4000, generation: 1 });
    expect(h.states()).toEqual(["starting", "ready"]);
    expect(h.sup.credentials).toEqual({ port: 4000, token, generation: 1 });
    // The control connection subscribed to the project events and asked which project is open.
    expect(h.rpcs[0]?.methods()).toEqual(["rpc.subscribe", "rpc.hello", "rpc.hello"]);
    // Status events never carry the token.
    expect(JSON.stringify(h.statuses)).not.toContain(token);
  });

  it("passes the start project with --project and does not recover it (a cold open)", async () => {
    const h = harness({ project: "/p/a.cutweave" });
    h.side.project = "/p/a.cutweave";
    h.side.recovery = true;
    void h.sup.start();
    await flush();
    expect(h.last().arg("--project")).toBe("/p/a.cutweave");
    await h.ready();
    expect(h.sup.status.state).toBe("ready");
    expect(h.rpcs[0]?.methods()).not.toContain("project.recover");
    expect(h.sup.status.recovery).toBeUndefined();
  });

  it("gives the ready line 10 s, then kills the engine and treats it as a crash", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await vi.advanceTimersByTimeAsync(9_999);
    expect(h.last().signals).toEqual([]);
    await vi.advanceTimersByTimeAsync(1);
    expect(h.last().signals).toEqual(["SIGKILL"]);
    h.last().exit(null, "SIGKILL");
    await flush();
    expect(h.sup.status).toMatchObject({
      state: "crashed",
      restartInMs: 500,
      lastError: { code: "ready_timeout" },
    });
    await vi.advanceTimersByTimeAsync(500);
    await flush();
    expect(h.children).toHaveLength(2);
  });

  it("reports a spawn error as failed without retrying", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    const child = h.last() as unknown as {
      pid: number | undefined;
      emit(e: string, x: Error): void;
    };
    Object.defineProperty(child, "pid", { value: undefined });
    child.emit("error", new Error("spawn engine-bin ENOENT"));
    await flush();
    expect(h.sup.status).toMatchObject({ state: "failed", lastError: { code: "spawn_failed" } });
    await vi.advanceTimersByTimeAsync(10_000);
    expect(h.children).toHaveLength(1);
  });
});

describe("crashes and backoff", () => {
  it("restarts after 0.5 s with a fresh token on the same port", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await h.ready();
    const first = h.sup.credentials;
    h.last().exit(null, "SIGKILL");
    await flush();
    expect(h.sup.status).toMatchObject({
      state: "crashed",
      restartInMs: 500,
      lastError: { code: "crashed", signal: "SIGKILL" },
    });
    expect(h.sup.credentials).toBeNull();
    await vi.advanceTimersByTimeAsync(499);
    expect(h.children).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    await flush();
    expect(h.children).toHaveLength(2);
    expect(h.sup.status.state).toBe("restarting");
    await h.ready();
    expect(h.sup.status).toMatchObject({ state: "ready", port: 4000, generation: 2 });
    const second = h.sup.credentials;
    expect(second?.port).toBe(4000);
    expect(second?.token).not.toBe(first?.token);
    expect(h.states()).toEqual(["starting", "ready", "crashed", "restarting", "ready"]);
  });

  it("backs off 0.5 s, 1 s, 2 s; the fourth crash within 60 s is failed", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await h.ready();
    const delays: number[] = [];
    for (let i = 0; i < 3; i++) {
      h.last().exit(70);
      await flush();
      expect(h.sup.status.state).toBe("crashed");
      const delay = h.sup.status.restartInMs ?? -1;
      delays.push(delay);
      await vi.advanceTimersByTimeAsync(delay - 1);
      expect(h.children).toHaveLength(i + 1);
      await vi.advanceTimersByTimeAsync(1);
      await flush();
      await h.ready();
      expect(h.sup.status.state).toBe("ready");
    }
    expect(delays).toEqual([500, 1_000, 2_000]);
    h.last().exit(70);
    await flush();
    expect(h.sup.status).toMatchObject({ state: "failed", lastError: { exitCode: 70 } });
    expect(h.sup.status.message).toMatch(/4 times in 60 s/);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.children).toHaveLength(4);

    // "Restart" in the dialog: a fresh budget.
    const settled = h.sup.restart();
    await flush();
    expect(h.children).toHaveLength(5);
    await h.ready();
    expect(await settled).toMatchObject({ state: "ready", generation: 5 });
    h.last().exit(70);
    await flush();
    expect(h.sup.status).toMatchObject({ state: "crashed", restartInMs: 500 });
  });

  it("forgets crashes older than 60 s", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await h.ready();
    for (let i = 0; i < 6; i++) {
      h.last().exit(70);
      await flush();
      expect(h.sup.status).toMatchObject({ state: "crashed", restartInMs: 500 });
      await vi.advanceTimersByTimeAsync(500);
      await flush();
      await h.ready();
      await vi.advanceTimersByTimeAsync(61_000);
    }
    expect(h.sup.status.state).toBe("ready");
  });

  it("counts an exit before ready (exit 1) as a crash and keeps the error line", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    h.last().errorLine("server_failed", 1);
    h.last().exit(1);
    await flush();
    expect(h.sup.status).toMatchObject({
      state: "crashed",
      lastError: { code: "server_failed", exitCode: 1, message: "server_failed!" },
    });
    await vi.advanceTimersByTimeAsync(500);
    await flush();
    expect(h.children).toHaveLength(2);
  });

  it("records the crash marker a panicking engine leaves", async () => {
    const h = harness({
      readCrashMarker: (pid) => (pid === 100 ? { message: "boom", location: "x.rs:1" } : null),
    });
    void h.sup.start();
    await flush();
    await h.ready();
    h.last().exit(101);
    await flush();
    expect(h.sup.status.lastError).toMatchObject({
      code: "crashed",
      panic: { message: "boom", location: "x.rs:1" },
    });
    expect(h.sup.status.lastError?.message).toMatch(/boom/);
  });
});

describe("start errors", () => {
  it.each([
    [2, "usage"],
    [10, "project_locked"],
    [11, "gpu_init_failed"],
    [13, "bad_handshake"],
  ])("exit %i (%s) is failed at once, with no restart", async (exitCode, code) => {
    const h = harness();
    void h.sup.start();
    await flush();
    const extra = exitCode === 10 ? { holder: { pid: 7, host: "H", started: 1 } } : {};
    h.last().errorLine(code, exitCode, extra);
    h.last().exit(exitCode);
    await flush();
    expect(h.sup.status).toMatchObject({ state: "failed", lastError: { code, exitCode } });
    if (exitCode === 10) {
      expect(h.sup.status.lastError?.holder).toEqual({ pid: 7, host: "H", started: 1 });
    }
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.children).toHaveLength(1);
  });

  it.each([
    [0xc0000135, "exit code 0xC0000135: a DLL it needs is missing"],
    [0xc0000139, "exit code 0xC0000139: a DLL it loaded is the wrong version"],
  ])(
    "Windows refusing to load the engine (%i) is failed at once and says why",
    async (exitCode, why) => {
      const h = harness();
      void h.sup.start();
      await flush();
      h.last().exit(exitCode);
      await flush();
      expect(h.sup.status).toMatchObject({
        state: "failed",
        lastError: {
          code: "start_failed",
          exitCode,
          message: `the engine exited before it was ready (${why})`,
        },
      });
      await vi.advanceTimersByTimeAsync(60_000);
      expect(h.children).toHaveLength(1);
    },
  );

  it("names other Windows status codes in hex and still restarts", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    h.last().exit(0xc0000005);
    await flush();
    expect(h.sup.status).toMatchObject({
      state: "crashed",
      lastError: {
        message: "the engine exited before it was ready (exit code 0xC0000005: access violation)",
      },
    });
    await vi.advanceTimersByTimeAsync(500);
    await flush();
    expect(h.children).toHaveLength(2);
  });

  it("exit 12 picks a new port, starts again at once and reports the change", async () => {
    const h = harness();
    const settled = h.sup.start();
    await flush();
    h.last().errorLine("port_in_use", 12);
    h.last().exit(12);
    await flush();
    expect(h.children).toHaveLength(2);
    expect(h.last().arg("--port")).toBe("5000");
    expect(h.portChanges).toEqual([[5000, 4000]]);
    await h.ready();
    expect(await settled).toMatchObject({ state: "ready", port: 5000 });
    expect(h.sup.credentials?.port).toBe(5000);
    expect(h.states()).toEqual(["starting", "ready"]);
  });

  it("gives up after three port retries in a row", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    for (let i = 0; i < 4; i++) {
      h.last().exit(12);
      await flush();
    }
    expect(h.children).toHaveLength(4);
    expect(h.sup.status).toMatchObject({ state: "failed", lastError: { exitCode: 12 } });
  });

  it("before a restart, moves off the previous port when something else took it", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await h.ready();
    h.busy.add(4000);
    h.last().exit(null, "SIGKILL");
    await flush();
    await vi.advanceTimersByTimeAsync(500);
    await flush();
    expect(h.last().arg("--port")).toBe("5000");
    expect(h.portChanges).toEqual([[5000, 4000]]);
    await h.ready();
    expect(h.sup.status).toMatchObject({ state: "ready", port: 5000 });
  });
});

describe("supervised restart and recovery", () => {
  it("reopens the open project and recovers it before reporting ready", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await h.ready();
    // The editor opened a project: main learns it from the project events.
    h.side.project = "/work/film.cutweave";
    h.rpcs[0]?.listener?.("project.opened", {});
    await flush();
    expect(h.sup.project).toBe("/work/film.cutweave");

    h.last().exit(null, "SIGKILL");
    // The journal holds unsaved work when the engine reopens the project.
    h.side.recovery = true;
    await flush();
    await vi.advanceTimersByTimeAsync(500);
    await flush();
    expect(h.last().arg("--project")).toBe("/work/film.cutweave");
    h.last().ready();
    await flush();
    const rpc = h.rpcs[1];
    expect(rpc?.methods()).toEqual(["rpc.subscribe", "rpc.hello", "project.recover", "rpc.hello"]);
    const recoverMeta = rpc?.calls[2]?.params?._meta as { request_id?: string } | undefined;
    const requestId = recoverMeta?.request_id;
    expect(requestId).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    expect(h.sup.status).toMatchObject({
      state: "ready",
      generation: 2,
      recovery: { project: "/work/film.cutweave", applied: 3, damaged: false },
    });
    // `ready` (and with it the renderer's credentials) only after the recovery.
    expect(h.states()).toEqual(["starting", "ready", "crashed", "restarting", "ready"]);
  });

  it("leaves a recovery decision that was pending at the crash to the user", async () => {
    const h = harness({ project: "/work/old.cutweave" });
    h.side.project = "/work/old.cutweave";
    h.side.recovery = true;
    void h.sup.start();
    await flush();
    await h.ready();
    h.last().exit(70);
    await flush();
    await vi.advanceTimersByTimeAsync(500);
    await flush();
    await h.ready();
    expect(h.last().arg("--project")).toBe("/work/old.cutweave");
    expect(h.rpcs[1]?.methods()).not.toContain("project.recover");
    expect(h.sup.status.state).toBe("ready");
  });

  it("reconnects its own connection when the engine closes it, and keeps tracking", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await h.ready();
    h.rpcs[0]?.drop();
    await vi.advanceTimersByTimeAsync(499);
    expect(h.rpcs).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    await flush();
    expect(h.rpcs).toHaveLength(2);
    expect(h.rpcs[1]?.methods()).toEqual(["rpc.subscribe", "rpc.hello"]);
    h.side.project = "/work/late.cutweave";
    h.rpcs[1]?.listener?.("project.opened", {});
    await flush();
    expect(h.sup.project).toBe("/work/late.cutweave");
    expect(h.sup.status.state).toBe("ready");
  });

  it("restarts without --project after the project was closed", async () => {
    const h = harness({ project: "/work/a.cutweave" });
    h.side.project = "/work/a.cutweave";
    void h.sup.start();
    await flush();
    await h.ready();
    h.side.project = null;
    h.rpcs[0]?.listener?.("project.closed", {});
    await flush();
    h.last().exit(70);
    await flush();
    await vi.advanceTimersByTimeAsync(500);
    await flush();
    expect(h.last().arg("--project")).toBeUndefined();
  });

  it("still reports ready when the control connection fails, flagging the missed recovery", async () => {
    let fail = false;
    const h = harness({
      connectRpc: async () => {
        if (fail) throw new Error("refused");
        return new FakeRpc({ project: "/w/p.cutweave", recovery: false });
      },
    });
    void h.sup.start();
    await flush();
    await h.ready();
    expect(h.sup.project).toBe("/w/p.cutweave");
    fail = true;
    h.last().exit(70);
    await flush();
    await vi.advanceTimersByTimeAsync(500);
    await flush();
    await h.ready();
    expect(h.sup.status).toMatchObject({
      state: "ready",
      lastError: { code: "recovery_failed" },
    });
    expect(h.sup.status.recovery).toBeUndefined();
  });
});

describe("stop", () => {
  it("closes stdin, then force-kills after 3 s; no restart afterwards", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await h.ready();
    const child = h.last();
    let stopped = false;
    const stopping = h.sup.stop().then(() => {
      stopped = true;
    });
    await flush();
    expect(child.stdinEnded).toBe(true);
    expect(child.signals).toEqual(process.platform === "win32" ? [] : ["SIGTERM"]);
    await vi.advanceTimersByTimeAsync(2_999);
    expect(child.signals).not.toContain("SIGKILL");
    await vi.advanceTimersByTimeAsync(1);
    expect(child.signals).toContain("SIGKILL");
    expect(stopped).toBe(false);
    child.exit(null, "SIGKILL");
    await stopping;
    expect(h.sup.status.state).toBe("stopped");
    expect(h.rpcs[0]?.closed).toBe(true);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(h.children).toHaveLength(1);
  });

  it("cancels a pending restart", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await h.ready();
    h.last().exit(70);
    await flush();
    expect(h.sup.status.state).toBe("crashed");
    await h.sup.stop();
    expect(h.sup.status.state).toBe("stopped");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(h.children).toHaveLength(1);
  });

  it("kill() is a SIGKILL of the running engine (test hook)", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await h.ready();
    expect(h.sup.kill()).toBe(100);
    expect(h.last().signals).toEqual(["SIGKILL"]);
  });
});

describe("window focus (WP-1J.04)", () => {
  const reports = (rpc: FakeRpc | undefined) =>
    rpc?.calls.filter((c) => c.method === "project.report_focus") ?? [];

  it("reports every focus gain to the engine, and nothing on a blur", async () => {
    const h = harness();
    void h.sup.start();
    await flush();
    await h.ready();
    expect(reports(h.rpcs[0])).toHaveLength(0);

    h.sup.setWindowFocused(true);
    await flush();
    h.sup.setWindowFocused(false);
    await flush();
    expect(reports(h.rpcs[0])).toHaveLength(1);
    h.sup.setWindowFocused(true);
    await flush();
    const sent = reports(h.rpcs[0]);
    expect(sent).toHaveLength(2);
    const meta = sent[0]?.params?._meta as { request_id?: string } | undefined;
    expect(meta?.request_id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-/);
  });

  it("tells an engine that starts under a focused window, also after a restart", async () => {
    const h = harness();
    h.sup.setWindowFocused(true);
    void h.sup.start();
    await flush();
    await h.ready();
    expect(reports(h.rpcs[0])).toHaveLength(1);

    h.last().exit(null, "SIGKILL");
    await flush();
    await vi.advanceTimersByTimeAsync(500);
    await flush();
    h.last().ready();
    await flush();
    expect(reports(h.rpcs[1])).toHaveLength(1);
  });

  it("does not tell an engine while the window is not focused", async () => {
    const h = harness();
    h.sup.setWindowFocused(false);
    void h.sup.start();
    await flush();
    await h.ready();
    expect(reports(h.rpcs[0])).toHaveLength(0);
  });
});

describe("folders agents can use (WP-1J.05)", () => {
  /** `--allow-root` values of a spawn, in order. */
  const roots = (child: FakeChild) =>
    child.args.flatMap((a, i) => (a === "--allow-root" ? [child.args[i + 1]] : []));
  const sent = (rpc: FakeRpc | undefined) =>
    (rpc?.calls ?? [])
      .filter((c) => c.method === "project.set_allowed_roots")
      .map((c) => c.params?.roots);

  it("passes every stored folder as --allow-root on start and after a supervised restart", async () => {
    const stored = ["/footage", "/gone", "/music"];
    const warnings: string[] = [];
    const exists = new Set(["/footage", "/music", "/dev-root"]);
    const h = harness({
      allowRoots: () =>
        engineAllowRoots(["/dev-root"], stored, {
          platform: "linux",
          isDirectory: (p) => exists.has(p),
          log: (m) => warnings.push(m),
        }),
    });
    void h.sup.start();
    await flush();
    // The folder that no longer exists is skipped, with a warning for the main log.
    expect(roots(h.last())).toEqual(["/dev-root", "/footage", "/music"]);
    expect(warnings).toEqual(["skipping a folder agents may use: /gone no longer exists"]);
    await h.ready();
    // Nothing changed while it started: no extra call.
    expect(sent(h.rpcs[0])).toEqual([]);

    // A crash: the restart reads the folders of today.
    stored.push("/new");
    exists.add("/new");
    h.last().exit(null, "SIGKILL");
    await flush();
    await vi.advanceTimersByTimeAsync(500);
    await flush();
    expect(h.children).toHaveLength(2);
    expect(roots(h.last())).toEqual(["/dev-root", "/footage", "/music", "/new"]);
  });

  it("sends a change to the running engine and waits for its answer", async () => {
    let folders = ["/a"];
    const h = harness({ allowRoots: () => folders });
    void h.sup.start();
    await flush();
    await h.ready();
    folders = [];
    await h.sup.applyAllowedRoots();
    expect(sent(h.rpcs[0])).toEqual([[]]);
    const meta = h.rpcs[0]?.calls.at(-1)?.params?._meta as { request_id?: string } | undefined;
    expect(meta?.request_id).toMatch(/^[0-9a-f]{8}-/);
  });

  it("gives an engine that was starting during a change the folders of today", async () => {
    let folders = ["/a"];
    const h = harness({ allowRoots: () => folders });
    void h.sup.start();
    await flush();
    expect(roots(h.last())).toEqual(["/a"]);
    // The person removes the folder before main is connected: nothing to send yet.
    folders = [];
    await h.sup.applyAllowedRoots();
    expect(h.rpcs).toHaveLength(0);
    await h.ready();
    expect(sent(h.rpcs[0])).toEqual([[]]);
    expect(h.sup.status.state).toBe("ready");
  });
});
