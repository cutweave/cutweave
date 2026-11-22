// SPDX-License-Identifier: GPL-3.0-or-later
// Placeholder engine supervisor (WP-1L.01). It spawns `cutweave engine --port P`, sends the
// per-launch UI token as the first stdin line (never argv/env/disk), waits for the readiness
// line and reports status. Crash/restart/backoff, exit-12 port re-pick and the failed dialog
// arrive in WP-1L.03.

import { type ChildProcessByStdio, spawn } from "node:child_process";
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";
import type { EngineState, EngineStatus } from "../../shared/api.ts";
import type { EngineLaunch } from "./launch.ts";

export const READY_TIMEOUT_MS = 10_000;
export const KILL_GRACE_MS = 3_000;

export interface EngineReady {
  readonly port: number;
  readonly pid: number;
  readonly version: string;
}

export interface SupervisorOptions {
  launch: EngineLaunch;
  port: number;
  /** Per-launch UI credential, delivered over the stdin handshake. */
  uiToken: string;
  env?: NodeJS.ProcessEnv;
  readyTimeoutMs?: number;
  killGraceMs?: number;
}

export type StatusListener = (status: EngineStatus) => void;

/** Parse the readiness line `{"event":"ready","port":P,"pid":N,"version":"…"}`; null otherwise. */
export function parseReadyLine(line: string): EngineReady | null {
  let data: unknown;
  try {
    data = JSON.parse(line);
  } catch {
    return null;
  }
  if (typeof data !== "object" || data === null) return null;
  const d = data as Record<string, unknown>;
  if (d.event !== "ready" || typeof d.port !== "number" || typeof d.pid !== "number") return null;
  return { port: d.port, pid: d.pid, version: typeof d.version === "string" ? d.version : "" };
}

export class EngineSupervisor {
  readonly #options: SupervisorOptions;
  readonly #listeners = new Set<StatusListener>();
  #child: ChildProcessByStdio<Writable, Readable, null> | null = null;
  #status: EngineStatus = { state: "unavailable" };
  #stopping = false;

  constructor(options: SupervisorOptions) {
    this.#options = options;
  }

  get status(): EngineStatus {
    return this.#status;
  }

  get pid(): number | undefined {
    return this.#child?.pid;
  }

  onStatus(listener: StatusListener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  #set(state: EngineState, extra: { port?: number; message?: string } = {}): void {
    this.#status = { state, ...extra };
    for (const listener of this.#listeners) listener(this.#status);
  }

  /** Spawn the engine and resolve once it printed the readiness line. Rejects on failure. */
  start(): Promise<EngineReady> {
    if (this.#child !== null) return Promise.reject(new Error("engine already started"));
    const { launch, port, uiToken } = this.#options;
    this.#stopping = false;
    this.#set("starting");

    return new Promise<EngineReady>((resolve, reject) => {
      let settled = false;
      const fail = (message: string) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        this.#set("failed", { message });
        this.#child?.kill();
        reject(new Error(message));
      };

      const child = spawn(
        launch.command,
        [...launch.args, "--port", String(port), "--token-stdin"],
        {
          stdio: ["pipe", "pipe", "inherit"],
          env: { ...process.env, ...this.#options.env },
          windowsHide: true,
        },
      );
      this.#child = child;
      const timer = setTimeout(
        () => fail("engine did not become ready in time"),
        this.#options.readyTimeoutMs ?? READY_TIMEOUT_MS,
      );

      child.once("error", (err) => {
        // A spawn failure emits no `exit`; drop the handle so `stop()` does not wait for one.
        this.#child = null;
        fail(`engine failed to start: ${err.message}`);
      });
      child.stdin.on("error", () => {
        // EPIPE when the engine already exited; the exit handler reports it.
      });
      child.stdin.write(`${uiToken}\n`);

      const lines = createInterface({ input: child.stdout });
      lines.on("line", (line) => {
        if (settled) return;
        const ready = parseReadyLine(line);
        if (ready === null) return;
        settled = true;
        clearTimeout(timer);
        this.#set("ready", { port: ready.port });
        resolve(ready);
      });

      child.once("exit", (code, signal) => {
        lines.close();
        this.#child = null;
        if (this.#stopping) {
          this.#set("stopped");
        } else if (!settled) {
          fail(`engine exited before ready (code ${code ?? signal})`);
        } else {
          this.#set("crashed", { message: `engine exited (code ${code ?? signal})` });
        }
      });
    });
  }

  /** Graceful stop: close the handshake pipe (engine exits on stdin EOF), force-kill after grace. */
  stop(): Promise<void> {
    const child = this.#child;
    if (child === null) return Promise.resolve();
    this.#stopping = true;
    return new Promise((resolve) => {
      const force = setTimeout(
        () => child.kill("SIGKILL"),
        this.#options.killGraceMs ?? KILL_GRACE_MS,
      );
      child.once("exit", () => {
        clearTimeout(force);
        resolve();
      });
      child.stdin.end();
    });
  }
}
