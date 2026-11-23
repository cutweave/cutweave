// SPDX-License-Identifier: GPL-3.0-or-later
// EngineSupervisor against the real engine binary (WP-1L.03): spawn → ready → SIGKILL →
// automatic restart with journal recovery → ready; the UI token never reaches disk, argv or
// env; old credentials die with the old engine; no engine outlives its supervisor.
//
// Needs the launch-protocol binary: `cargo build -p cutweave-engine --bin cutweave-engine-serve`
// (or CUTWEAVE_ENGINE_BIN). Skipped when it is not built.

import { type ChildProcess, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { EngineStatus } from "../../shared/api.ts";
import { pickFreePort } from "./ports.ts";
import { connectEngineRpc, type EngineRpc, EngineRpcError, uuidv7 } from "./rpc-client.ts";
import { type ChildLike, EngineSupervisor, type SpawnFn } from "./supervisor.ts";

const here = path.dirname(fileURLToPath(import.meta.url));
const desktopDir = path.resolve(here, "..", "..", "..");
const repoRoot = path.resolve(desktopDir, "..", "..");
const exe = process.platform === "win32" ? ".exe" : "";
const engineBin =
  process.env.CUTWEAVE_ENGINE_BIN ??
  path.join(repoRoot, "target", "debug", `cutweave-engine-serve${exe}`);
const available = existsSync(engineBin);

/** Every file under `dir`, recursively. */
function filesUnder(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...filesUnder(p));
    else if (entry.isFile()) out.push(p);
  }
  return out;
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/** The engine's run-file (agent tokens; never the UI token). */
function runFile(home: string, pid: number): { agent_tokens: { token: string }[]; port: number } {
  return JSON.parse(readFileSync(path.join(home, ".cutweave", "run", `${pid}.json`), "utf8"));
}

/**
 * No file contains a token. While the engine runs, Windows refuses reads of the OS-locked
 * project `lock.guard` (EBUSY; it is empty); only that file may be skipped, unless `all` is set.
 */
function expectNoToken(files: string[], tokens: string[], all = false): void {
  for (const file of files) {
    let bytes: Buffer;
    try {
      bytes = readFileSync(file);
    } catch (err) {
      if (!all && path.basename(file) === "lock.guard") continue;
      throw new Error(`cannot read ${file}: ${err instanceof Error ? err.message : err}`);
    }
    for (const token of tokens) expect(bytes.includes(token), file).toBe(false);
  }
}

/** Tracks in `sequence` per the live document (`project.snapshot`). */
async function trackCount(rpc: EngineRpc, sequence: string): Promise<number> {
  const snap = (await rpc.call("project.snapshot")) as {
    project: { sequences: Record<string, { tracks: Record<string, unknown> }> };
  };
  return Object.keys(snap.project.sequences[sequence]?.tracks ?? {}).length;
}

interface Spawned {
  args: readonly string[];
  env: NodeJS.ProcessEnv;
  pid: number | undefined;
}

// Spawns and kills the real engine binary: on a machine saturated by parallel builds a first-run
// executable scan can delay its ready line past the supervisor timeout. Retry once; a real
// regression fails both attempts.
describe.skipIf(!available)("EngineSupervisor with the real engine", { retry: 1 }, () => {
  let home = "";
  let sup: EngineSupervisor | null = null;
  const conns: EngineRpc[] = [];
  const spawned: Spawned[] = [];

  /** Records what each engine was started with (argv and env as the OS receives them). */
  const recordingSpawn: SpawnFn = (command, args, options) => {
    const child = spawn(command, [...args], {
      stdio: ["pipe", "pipe", "inherit"],
      env: options.env,
      windowsHide: true,
    });
    spawned.push({ args, env: options.env, pid: child.pid });
    return child as unknown as ChildLike;
  };

  const connect = async (port: number, token: string) => {
    const rpc = await connectEngineRpc(port, token);
    conns.push(rpc);
    return rpc;
  };
  const meta = () => ({ _meta: { request_id: uuidv7() } });

  beforeEach(() => {
    home = mkdtempSync(path.join(tmpdir(), "cw-supervisor-"));
    spawned.length = 0;
  });
  afterEach(async () => {
    for (const c of conns.splice(0)) c.close();
    await sup?.stop();
    sup = null;
    rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  });

  it("restarts a SIGKILLed engine, recovers the journal, and rotates the credentials", async () => {
    sup = new EngineSupervisor({
      launch: { command: engineBin, args: ["engine"] },
      port: await pickFreePort(),
      env: { HOME: home, USERPROFILE: home },
      extraArgs: ["--allow-origin", "app://cutweave"],
      spawn: recordingSpawn,
      log: () => {},
    });
    const seen: EngineStatus[] = [];
    sup.onStatus((s) => seen.push(s));
    expect(await sup.start()).toMatchObject({ state: "ready", generation: 1 });
    const first = sup.credentials;
    if (first === null) throw new Error("no credentials while ready");
    const firstPid = sup.pid ?? 0;
    expect(alive(firstPid)).toBe(true);

    // The editor saves the untitled document as a project and makes three edits.
    const ui = await connect(first.port, first.token);
    expect(await ui.call("rpc.hello", { client: "test", version: "0" })).toMatchObject({
      actor: "user",
    });
    const { sequence } = (await ui.call("project.new", { name: "Film", ...meta() })) as {
      sequence: string;
    };
    const folder = path.join(home, "projects", "film");
    mkdirSync(path.dirname(folder), { recursive: true });
    const saved = (await ui.call("project.save_as", { path: folder, ...meta() })) as {
      path: string;
    };
    await expect.poll(() => sup?.project).toBe(saved.path);
    const tracksBefore = await trackCount(ui, sequence);
    expect(tracksBefore).toBe(2);
    for (let i = 0; i < 3; i++) {
      await ui.call("track.add", { sequence, kind: "video", ...meta() });
    }
    const oldAgentToken = runFile(home, firstPid).agent_tokens[0]?.token ?? "";
    expect(oldAgentToken).not.toBe("");

    // Crash: SIGKILL, well inside autosave's 2 s idle window (the edits are journaled only).
    expect(sup.kill()).toBe(firstPid);
    await expect
      .poll(() => (sup?.status.generation === 2 ? sup.status.state : "waiting"), {
        timeout: 30_000,
        interval: 100,
      })
      .toBe("ready");
    expect(seen.map((s) => s.state)).toEqual([
      "starting",
      "ready",
      "crashed",
      "restarting",
      "ready",
    ]);
    expect(seen.find((s) => s.state === "crashed")?.lastError).toMatchObject({
      code: "crashed",
    });
    expect(alive(firstPid)).toBe(false);

    // Reopened with --project and recovered by main, without a prompt.
    expect(spawned[1]?.args).toContain("--project");
    expect(sup.status.recovery).toEqual({ project: saved.path, applied: 3, damaged: false });
    const second = sup.credentials;
    if (second === null) throw new Error("no credentials after the restart");
    expect(second.port).toBe(first.port);
    expect(second.token).not.toBe(first.token);

    // The new engine takes the new UI token: the three edits are there exactly once.
    const ui2 = await connect(second.port, second.token);
    const hello = (await ui2.call("rpc.hello", { client: "test", version: "0" })) as {
      actor: string;
      project: { path: string; recovery: unknown };
    };
    expect(hello).toMatchObject({ actor: "user", project: { path: saved.path, recovery: null } });
    expect(await trackCount(ui2, sequence)).toBe(tracksBefore + 3);

    // The old credentials died with the old engine.
    await expect(connectEngineRpc(second.port, first.token)).rejects.toThrow();
    await expect(connectEngineRpc(second.port, oldAgentToken)).rejects.toThrow();
    // The new engine's agent token is an agent: no UI-only privileges (here: paths outside
    // the project and the approved folders, which only the UI credential may use).
    const newAgentToken = runFile(home, sup.pid ?? 0).agent_tokens[0]?.token ?? "";
    const agent = await connect(second.port, newAgentToken);
    const agentHello = (await agent.call("rpc.hello", { client: "test", version: "0" })) as {
      actor: string;
    };
    expect(agentHello.actor).toMatch(/^agent:/);
    const elsewhere = path.join(home, "elsewhere");
    const refused = await agent
      .call("project.open", { path: elsewhere, ...meta() })
      .catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(EngineRpcError);
    expect((refused as EngineRpcError).dataCode).toBe("invalid");

    // The UI tokens were never written anywhere under HOME (run-files, logs, the project and
    // its journal) and never passed in argv or the environment.
    const files = filesUnder(home);
    expect(files.some((f) => f.includes(`${path.sep}run${path.sep}`))).toBe(true);
    expect(files.some((f) => f.includes(`${path.sep}journal${path.sep}`))).toBe(true);
    expectNoToken(files, [first.token, second.token]);
    for (const token of [first.token, second.token]) {
      for (const s of spawned) {
        expect(s.args.join(" ")).not.toContain(token);
        expect(Object.values(s.env).join("\n")).not.toContain(token);
      }
    }
    // Where the OS exposes another process's argv and environment, check those too.
    const pid = sup.pid ?? 0;
    if (existsSync(`/proc/${pid}/environ`)) {
      for (const part of ["cmdline", "environ"]) {
        const text = readFileSync(`/proc/${pid}/${part}`, "utf8");
        expect(text).not.toContain(second.token);
      }
    }

    // Quit: the engine stops and leaves no process behind.
    await sup.stop();
    expect(sup.status.state).toBe("stopped");
    expect(alive(pid)).toBe(false);
    // Once nothing holds a lock, every file left under HOME is checked again.
    expectNoToken(filesUnder(home), [first.token, second.token], true);
  }, 90_000);

  it("CUTWEAVE_TEST_CRASH_AFTER aborts after N commands; the restart recovers them", async () => {
    sup = new EngineSupervisor({
      launch: { command: engineBin, args: ["engine"] },
      port: await pickFreePort(),
      env: { HOME: home, USERPROFILE: home, CUTWEAVE_TEST_CRASH_AFTER: "3" },
      log: () => {},
    });
    await sup.start();
    const creds = sup.credentials;
    if (creds === null) throw new Error("not ready");
    const ui = await connect(creds.port, creds.token);
    // Commit 1 (saved by save_as below), then commits 2 and 3 (journaled only).
    const { sequence } = (await ui.call("project.new", { name: "P", ...meta() })) as {
      sequence: string;
    };
    await ui.call("project.save_as", { path: path.join(home, "p"), ...meta() });
    await expect.poll(() => sup?.project).not.toBeNull();
    await ui.call("track.add", { sequence, kind: "video", ...meta() });
    // The third commit aborts the engine before its reply leaves.
    const lost = await ui
      .call("track.add", { sequence, kind: "audio", ...meta() })
      .catch(() => "lost");
    expect(lost).toBe("lost");
    await expect
      .poll(() => (sup?.status.generation === 2 ? sup.status.state : "waiting"), {
        timeout: 30_000,
        interval: 100,
      })
      .toBe("ready");
    // Both journaled commits, including the unacknowledged one, were restored by main (the
    // hook's counter restarts with the engine).
    expect(sup.status.recovery?.applied).toBe(2);
    expect(sup.status.lastError?.code).toBe("crashed");
    const after = sup.credentials;
    if (after === null) throw new Error("not ready after the restart");
    expect(await trackCount(await connect(after.port, after.token), sequence)).toBe(4);
  }, 90_000);

  it("an engine never outlives its supervisor process (stdin EOF on parent death)", async () => {
    const harness = path.join(desktopDir, "test-support", "supervised-engine.ts");
    const parent: ChildProcess = spawn(
      process.execPath,
      [harness, engineBin, String(await pickFreePort())],
      {
        stdio: ["ignore", "pipe", "inherit"],
        env: { ...process.env, HOME: home, USERPROFILE: home },
        windowsHide: true,
      },
    );
    const lines = createInterface({ input: parent.stdout as NodeJS.ReadableStream });
    const first = await new Promise<string>((resolve) => lines.once("line", resolve));
    const { engine } = JSON.parse(first) as { engine?: number };
    if (engine === undefined) throw new Error(`harness failed: ${first}`);
    expect(alive(engine)).toBe(true);

    parent.kill("SIGKILL");
    await expect.poll(() => alive(engine), { timeout: 15_000, interval: 100 }).toBe(false);
  }, 60_000);
});
