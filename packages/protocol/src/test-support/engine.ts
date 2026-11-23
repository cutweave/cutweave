// SPDX-License-Identifier: GPL-3.0-or-later
// Test support (Node only, never part of the package build): starts a real headless engine
// process (`cutweave-engine-serve`, the launch protocol of docs/reference/launch-protocol.md)
// for integration tests in this package and in `packages/editor`.
//
// The engine runs in Gui mode so the test connects with the UI credential like the app does:
// the token is the first line on stdin, never argv or the environment. Run-file, logs and HOME
// live in a temp directory that is removed on `stop()`.

import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { ConnectionInfo } from "../connection.ts";

const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));

export interface EngineBinary {
  readonly path: string;
  /** Set when there is no usable binary; the tests skip and print this. */
  readonly unavailable?: undefined;
}

export interface EngineUnavailable {
  readonly unavailable: string;
}

/**
 * Finds the engine binary: `CUTWEAVE_ENGINE_BIN` if set, otherwise the result of
 * `cargo build -p cutweave-engine --bin cutweave-engine-serve` (a no-op when up to date).
 * Returns the reason instead of throwing when neither works, so callers can skip cleanly.
 */
export function resolveEngineBinary(): EngineBinary | EngineUnavailable {
  const fromEnv = process.env.CUTWEAVE_ENGINE_BIN;
  if (fromEnv !== undefined && fromEnv !== "") {
    return { path: fromEnv };
  }
  const build = spawnSync(
    "cargo",
    [
      "build",
      "-p",
      "cutweave-engine",
      "--bin",
      "cutweave-engine-serve",
      "--message-format=json-render-diagnostics",
    ],
    { cwd: REPO_ROOT, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, timeout: 20 * 60_000 },
  );
  if (build.error !== undefined) {
    return { unavailable: `could not run cargo: ${build.error.message}` };
  }
  if (build.status !== 0) {
    return { unavailable: `cargo build of cutweave-engine-serve failed (exit ${build.status})` };
  }
  let found: string | undefined;
  for (const line of build.stdout.split("\n")) {
    if (!line.startsWith("{")) {
      continue;
    }
    try {
      const message = JSON.parse(line) as {
        reason?: string;
        target?: { name?: string };
        executable?: string | null;
      };
      if (
        message.reason === "compiler-artifact" &&
        message.target?.name === "cutweave-engine-serve" &&
        typeof message.executable === "string"
      ) {
        found = message.executable;
      }
    } catch {
      // not a cargo message
    }
  }
  return found === undefined
    ? { unavailable: "cargo did not report a cutweave-engine-serve executable" }
    : { path: found };
}

export interface EngineHandle {
  readonly port: number;
  readonly pid: number;
  /** The UI credential this engine was started with. */
  readonly token: string;
  readonly info: ConnectionInfo;
  /** Closes stdin (the engine stops when it closes), waits for the exit, removes the temp dir. */
  stop(): Promise<void>;
}

function readReadyLine(
  child: ChildProcess,
  timeoutMs: number,
): Promise<{ port: number; pid: number }> {
  return new Promise((resolve, reject) => {
    let buffer = "";
    let stderr = "";
    const timer = setTimeout(() => {
      reject(new Error(`engine not ready after ${timeoutMs} ms; stderr: ${stderr}`));
    }, timeoutMs);
    child.stderr?.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
    });
    child.stdout?.on("data", (chunk: Buffer) => {
      buffer += chunk.toString();
      for (const line of buffer.split("\n")) {
        if (!line.startsWith("{")) {
          continue;
        }
        try {
          const message = JSON.parse(line) as {
            event?: string;
            port?: number;
            pid?: number;
            message?: string;
          };
          if (
            message.event === "ready" &&
            message.port !== undefined &&
            message.pid !== undefined
          ) {
            clearTimeout(timer);
            resolve({ port: message.port, pid: message.pid });
            return;
          }
          if (message.event === "error") {
            clearTimeout(timer);
            reject(new Error(`engine failed to start: ${line}`));
            return;
          }
        } catch {
          // an incomplete line: wait for more
        }
      }
    });
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`engine exited with ${code} before it was ready; stderr: ${stderr}`));
    });
  });
}

/** Starts a Gui-mode engine on a free port with an untitled document. */
export async function startEngine(
  binary: string,
  extraArgs: readonly string[] = [],
): Promise<EngineHandle> {
  const dir = mkdtempSync(join(tmpdir(), "cutweave-engine-test-"));
  const token = randomBytes(32).toString("hex");
  const child = spawn(
    binary,
    [
      "--port",
      "0",
      "--run-dir",
      join(dir, "run"),
      "--no-log",
      "--allow-origin",
      "app://cutweave",
      ...extraArgs,
    ],
    {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, HOME: dir, USERPROFILE: dir },
    },
  );
  const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
  child.stdin?.write(`${token}\n`);
  let ready: { port: number; pid: number };
  try {
    ready = await readReadyLine(child, 30_000);
  } catch (error) {
    child.kill();
    rmSync(dir, { recursive: true, force: true });
    throw error;
  }
  return {
    port: ready.port,
    pid: ready.pid,
    token,
    info: { wsUrl: `ws://127.0.0.1:${ready.port}/rpc`, token },
    async stop() {
      child.stdin?.end();
      const killer = setTimeout(() => child.kill(), 5_000);
      await exited;
      clearTimeout(killer);
      rmSync(dir, { recursive: true, force: true });
    },
  };
}
