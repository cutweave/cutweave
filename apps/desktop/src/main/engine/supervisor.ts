// SPDX-License-Identifier: GPL-3.0-or-later
// EngineSupervisor (WP-1L.03): runs `cutweave engine` as a child of Electron main and keeps it
// running. Contract: docs/reference/launch-protocol.md, arch §2, §6, §17, §18, ADR-0016.
//
// Per launch: a fresh 256-bit UI token goes to the engine as the first stdin line (never argv,
// env, disk or logs) and to the renderer only through the preload getter (`credentials`);
// `--port` is the port main chose (the CSP names it). The supervisor waits for the ready line
// (10 s), then opens its own JSON-RPC connection to learn which project is open.
//
// States: starting → ready → crashed → restarting → ready …, failed, stopped.
//   - Exit after ready = crash. Restarts follow 0.5 s, 1 s, 2 s; a fourth crash within 60 s
//     (more than three restarts) gives up: `failed`, and main offers Restart / Quit.
//   - Start errors: 12 (port taken) picks a new port and starts again at once (the CSP
//     follows, `onPortChange`); 2, 10, 11, 13 and a spawn error are `failed` at once (a retry
//     cannot fix them); anything else (exit 1, a signal, no ready line in 10 s) counts as a crash.
//   - Before every restart the previous port is checked; if it is taken, a new one is picked.
//
// Supervised restart vs. cold open (arch §17): a restart reopens the project the crashed
// engine held (`--project`) and, before reporting `ready`, calls `project.recover` itself:
// the journal is replayed with no prompt, because it holds the work of the session the user
// was just in. A user's cold open of a dirty project (`project.open` from the editor) instead
// gets `project.recovery_available` and the restore/discard dialog (WP-1M.07). If that dialog
// was still pending when the engine crashed, the restart leaves the decision to the user too.
//
// Orphans: in Gui mode the engine stops when its stdin closes, which happens when this process
// exits for any reason. Quit closes stdin (and sends SIGTERM on POSIX), then force-kills after 3 s.

import { type ChildProcessByStdio, spawn as nodeSpawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";
import type {
  EngineErrorInfo,
  EngineRecovery,
  EngineState,
  EngineStatus,
} from "../../shared/api.ts";
import type { EngineLaunch } from "./launch.ts";
import { isPortFree, pickFreePort } from "./ports.ts";
import {
  defaultEngineLogDir,
  describeExitCode,
  type EngineErrorLine,
  type EngineReady,
  EXIT,
  parseEngineLine,
  readCrashMarker,
  WINDOWS_LOADER_EXITS,
} from "./protocol.ts";
import { connectEngineRpc, type EngineRpc, uuidv7 } from "./rpc-client.ts";

export const READY_TIMEOUT_MS = 10_000;
export const KILL_GRACE_MS = 3_000;
/** Delay before restart n (1-based) within the crash window; its length is the restart budget. */
export const RESTART_BACKOFF_MS: readonly number[] = [500, 1_000, 2_000];
export const CRASH_WINDOW_MS = 60_000;
/** Consecutive exit-12 retries on a fresh port before giving up. */
export const MAX_PORT_RETRIES = 3;
/** Delay before main reconnects its own engine connection after the engine closed it. */
const CONTROL_RECONNECT_MS = 500;

/** Exit codes a retry cannot fix: `failed` at once. */
const FATAL_EXITS = new Set<number>([
  EXIT.usage,
  EXIT.projectLocked,
  EXIT.gpuInitFailed,
  EXIT.badHandshake,
  // Windows refused to load the engine (a missing or mismatched DLL): it never ran.
  WINDOWS_LOADER_EXITS.dllNotFound,
  WINDOWS_LOADER_EXITS.entryPointNotFound,
]);

/** Project events after which main re-reads which project the engine holds. */
const PROJECT_EVENTS = [
  "project.opened",
  "project.closed",
  "project.saved",
  "project.recovery_available",
  "doc.reset",
];

/** The parts of a spawned process the supervisor uses (tests pass a fake). */
export interface ChildLike {
  readonly pid?: number | undefined;
  readonly stdin: Writable;
  readonly stdout: Readable;
  kill(signal?: NodeJS.Signals): boolean;
  once(event: "exit", listener: (code: number | null, signal: NodeJS.Signals | null) => void): this;
  once(event: "error", listener: (err: Error) => void): this;
}

export type SpawnFn = (
  command: string,
  args: readonly string[],
  options: { env: NodeJS.ProcessEnv },
) => ChildLike;

export interface SupervisorOptions {
  launch: EngineLaunch;
  /** Port main chose at app launch (the CSP is built from it). */
  port: number;
  /** Project to open at the first start (`--project`). */
  project?: string | null;
  /** Arguments after the per-launch ones (`--allow-origin`, `--run-dir`, `--log-dir`, …). */
  extraArgs?: readonly string[];
  /**
   * The folders agents may use (WP-1J.05), read at every start and supervised restart (each one
   * an `--allow-root`) and sent to the running engine by `applyAllowedRoots`.
   */
  allowRoots?(): readonly string[];
  /** Merged over `process.env` for the engine. Never put the token here. */
  env?: NodeJS.ProcessEnv;
  /** Sent in `rpc.hello`. */
  clientVersion?: string;
  readyTimeoutMs?: number;
  killGraceMs?: number;
  backoffMs?: readonly number[];
  crashWindowMs?: number;
  maxPortRetries?: number;
  /** Called before an engine starts on another port: main updates the CSP's port. */
  onPortChange?(port: number, previous: number): void;
  /** Supervisor diagnostics (never contain a token). */
  log?(message: string): void;
  // Seams for tests; defaults are the real ones.
  spawn?: SpawnFn;
  generateToken?(): string;
  isPortFree?(port: number): Promise<boolean>;
  pickPort?(): Promise<number>;
  connectRpc?(port: number, token: string): Promise<EngineRpc>;
  readCrashMarker?(pid: number): EngineErrorInfo["panic"] | null;
  now?(): number;
}

export type StatusListener = (status: EngineStatus) => void;

/** One engine process. */
interface Run {
  readonly child: ChildLike;
  readonly token: string;
  readonly generation: number;
  /** Started by a supervised or dialog restart (not the app's first start). */
  readonly restart: boolean;
  /** Run journal recovery on the reopened project before reporting `ready`. */
  readonly recover: boolean;
  readonly project: string | null;
  /** The folders it was started with (`--allow-root`). */
  readonly roots: readonly string[];
  readyTimer: ReturnType<typeof setTimeout> | undefined;
  /** The ready line arrived. */
  sawReady: boolean;
  timedOut: boolean;
  exited: boolean;
  errorLine: EngineErrorLine | null;
}

/** 256-bit UI credential as 64 hex characters (the engine accepts 32–512 printable ASCII). */
export function generateUiToken(): string {
  return randomBytes(32).toString("hex");
}

const defaultSpawn: SpawnFn = (command, args, options) =>
  nodeSpawn(command, [...args], {
    // stdin: the handshake pipe (token, then EOF = stop); stdout: JSON lines; stderr: human.
    stdio: ["pipe", "pipe", "inherit"],
    env: options.env,
    windowsHide: true,
  }) as ChildProcessByStdio<Writable, Readable, null>;

interface HelloProject {
  path: string | null;
  recoveryPending: boolean;
}

function helloProject(result: unknown): HelloProject {
  const project = (result as { project?: { path?: unknown; recovery?: unknown } } | null)?.project;
  return {
    path: typeof project?.path === "string" ? project.path : null,
    recoveryPending: project?.recovery !== null && project?.recovery !== undefined,
  };
}

export class EngineSupervisor {
  readonly #o: SupervisorOptions;
  readonly #listeners = new Set<StatusListener>();
  #status: EngineStatus = { state: "stopped" };
  #port: number;
  #run: Run | null = null;
  #rpc: EngineRpc | null = null;
  #generation = 0;
  /** Bumped by every launch, stop and restart: stale async work compares and bails out. */
  #epoch = 0;
  #started = false;
  #stopping = false;
  #stopWaiters: (() => void)[] = [];
  #restartTimer: ReturnType<typeof setTimeout> | undefined;
  #crashes: number[] = [];
  #portRetries = 0;
  #lastError: EngineErrorInfo | undefined;
  #recovery: EngineRecovery | undefined;
  /** The project the engine holds, as far as main knows (reopened after a crash). */
  #project: string | null;
  /** A recovery decision was pending in the editor when the engine went down. */
  #recoveryPending = false;
  /** Whether the app window has the focus; a new engine is told at once (WP-1J.04). */
  #windowFocused = false;

  constructor(options: SupervisorOptions) {
    this.#o = options;
    this.#port = options.port;
    this.#project = options.project ?? null;
  }

  get status(): EngineStatus {
    return this.#status;
  }

  get port(): number {
    return this.#port;
  }

  get pid(): number | undefined {
    return this.#run?.child.pid;
  }

  /** The project main would reopen after a crash. */
  get project(): string | null {
    return this.#project;
  }

  /** Port, UI token and launch generation of the running engine while `ready`; else null. */
  get credentials(): { port: number; token: string; generation: number } | null {
    if (this.#status.state !== "ready" || this.#run === null) return null;
    return { port: this.#port, token: this.#run.token, generation: this.#run.generation };
  }

  onStatus(listener: StatusListener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /**
   * The window gained or lost the focus. Gaining it is reported to the engine
   * (`project.report_focus`, UI credential only), which records the time in its run-file: an
   * agent that does not say which engine it means gets the one whose window was focused last
   * (WP-1J.04). An engine that starts, or restarts, under a focused window is told as soon as
   * main is connected.
   */
  setWindowFocused(focused: boolean): void {
    this.#windowFocused = focused;
    if (focused) this.#reportFocus();
  }

  #reportFocus(): void {
    const rpc = this.#rpc;
    if (rpc === null) return;
    rpc
      .call("project.report_focus", { _meta: { request_id: uuidv7() } })
      .catch((err: unknown) => this.#log(`reporting the window focus failed: ${errorText(err)}`));
  }

  /**
   * The folders agents may use changed (WP-1J.05): tells the running engine at once
   * (`project.set_allowed_roots`, UI credential only), so a removed folder stops agents that are
   * already connected. Resolves when the engine answered; with no engine connected it resolves at
   * once (the next start passes the folders as `--allow-root`, and a start already under way
   * sends them as soon as main is connected).
   */
  async applyAllowedRoots(): Promise<void> {
    const rpc = this.#rpc;
    if (rpc === null) return;
    await this.#sendAllowedRoots(rpc, this.#o.allowRoots?.() ?? []);
  }

  async #sendAllowedRoots(rpc: EngineRpc, roots: readonly string[]): Promise<void> {
    try {
      await rpc.call("project.set_allowed_roots", {
        roots: [...roots],
        _meta: { request_id: uuidv7() },
      });
    } catch (err) {
      this.#log(`the folders agents may use were not applied: ${errorText(err)}`);
    }
  }

  /** Resolves with the next `ready`, `failed` or `stopped` status. */
  whenSettled(): Promise<EngineStatus> {
    return new Promise((resolve) => {
      const off = this.onStatus((s) => {
        if (s.state === "ready" || s.state === "failed" || s.state === "stopped") {
          off();
          resolve(s);
        }
      });
    });
  }

  /** Start the first engine; resolves once it is ready or has failed. */
  start(): Promise<EngineStatus> {
    if (this.#started) return Promise.reject(new Error("the engine supervisor already started"));
    this.#started = true;
    const settled = this.whenSettled();
    void this.#launch("start");
    return settled;
  }

  /** "Restart" from the failed dialog: a fresh crash budget, the project reopened and recovered. */
  restart(): Promise<EngineStatus> {
    if (this.#status.state !== "failed" || this.#stopping) return Promise.resolve(this.#status);
    this.#crashes = [];
    this.#portRetries = 0;
    const settled = this.whenSettled();
    void this.#launch("restart");
    return settled;
  }

  /**
   * Test hook: forget past crashes, so a suite that crashes the engine on purpose many times
   * (WP-1L.03 resend tests) is not cut short by the crash budget.
   */
  resetCrashHistory(): void {
    this.#crashes = [];
  }

  /** Test hook: SIGKILL the engine (a simulated crash). Returns the killed pid, or null. */
  kill(): number | null {
    const run = this.#run;
    if (run === null || run.exited) return null;
    run.child.kill("SIGKILL");
    return run.child.pid ?? null;
  }

  /**
   * Quit: close the handshake pipe (the engine stops on stdin EOF; also SIGTERM on POSIX),
   * force-kill after the grace period. Resolves when the process has exited.
   */
  stop(): Promise<void> {
    this.#stopping = true;
    this.#epoch++;
    clearTimeout(this.#restartTimer);
    this.#closeRpc();
    const run = this.#run;
    if (run === null || run.exited) {
      this.#set("stopped");
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      this.#stopWaiters.push(resolve);
      if (this.#stopWaiters.length > 1) return;
      const force = setTimeout(() => {
        this.#log("engine did not stop in time; killing it");
        run.child.kill("SIGKILL");
      }, this.#o.killGraceMs ?? KILL_GRACE_MS);
      this.#stopWaiters.push(() => clearTimeout(force));
      run.child.stdin.end();
      if (process.platform !== "win32") run.child.kill("SIGTERM");
    });
  }

  // ----- launching -------------------------------------------------------------------------

  async #launch(kind: "start" | "restart"): Promise<void> {
    const epoch = ++this.#epoch;
    this.#recovery = undefined;
    this.#set(kind === "start" ? "starting" : "restarting");
    if (kind === "restart") {
      // The previous port may have been taken while the engine was down.
      const free = await (this.#o.isPortFree ?? isPortFree)(this.#port).catch(() => false);
      if (epoch !== this.#epoch) return;
      if (!free && !(await this.#changePort(epoch))) return;
    }
    this.#spawn(kind === "restart");
  }

  /** Pick a new port and tell main; false when superseded meanwhile. */
  async #changePort(epoch: number): Promise<boolean> {
    const pick = this.#o.pickPort ?? (() => pickFreePort());
    let next: number;
    try {
      next = await pick();
      if (next === this.#port) next = await pick();
    } catch (err) {
      if (epoch === this.#epoch) {
        this.#fail({ code: "start_failed", message: `no free port: ${errorText(err)}` });
      }
      return false;
    }
    if (epoch !== this.#epoch) return false;
    const previous = this.#port;
    this.#port = next;
    this.#log(`engine port ${previous} is taken; using ${next}`);
    this.#o.onPortChange?.(next, previous);
    return true;
  }

  /** `restart`: a supervised (or dialog) restart, which recovers the reopened project. */
  #spawn(restart: boolean): void {
    // A fresh credential for every engine process: the old one dies with the old engine.
    const token = (this.#o.generateToken ?? generateUiToken)();
    const project = this.#project;
    // The folders agents may use, read now: a restart gets the list as it is today.
    const roots = [...(this.#o.allowRoots?.() ?? [])];
    const args = [
      ...this.#o.launch.args,
      "--port",
      String(this.#port),
      "--token-stdin",
      ...(project === null ? [] : ["--project", project]),
      ...(this.#o.extraArgs ?? []),
      ...roots.flatMap((root) => ["--allow-root", root]),
    ];
    const env = { ...process.env, ...this.#o.env };
    let child: ChildLike;
    try {
      child = (this.#o.spawn ?? defaultSpawn)(this.#o.launch.command, args, { env });
    } catch (err) {
      this.#fail({
        code: "spawn_failed",
        message: `the engine could not start: ${errorText(err)}`,
      });
      return;
    }
    const run: Run = {
      child,
      token,
      generation: ++this.#generation,
      restart,
      recover: restart && project !== null && !this.#recoveryPending,
      project,
      roots,
      readyTimer: undefined,
      sawReady: false,
      timedOut: false,
      exited: false,
      errorLine: null,
    };
    this.#run = run;

    child.stdin.on("error", () => {
      // EPIPE when the engine already exited; the exit handler reports it.
    });
    // The handshake: the UI credential is the first stdin line.
    child.stdin.write(`${token}\n`);
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => this.#onLine(run, line));
    child.once("error", (err) => {
      // A spawn failure (no such file) emits no `exit`.
      if (child.pid !== undefined || run.exited) return;
      run.exited = true;
      clearTimeout(run.readyTimer);
      lines.close();
      if (this.#run !== run) return;
      this.#run = null;
      this.#fail({ code: "spawn_failed", message: `the engine could not start: ${err.message}` });
    });
    child.once("exit", (code, signal) => {
      lines.close();
      this.#onExit(run, code, signal);
    });
    run.readyTimer = setTimeout(() => {
      if (run.sawReady || run.exited) return;
      run.timedOut = true;
      child.kill("SIGKILL");
    }, this.#o.readyTimeoutMs ?? READY_TIMEOUT_MS);
  }

  #onLine(run: Run, line: string): void {
    const parsed = parseEngineLine(line);
    if (parsed === null || run.exited) return;
    if (parsed.event === "error") {
      run.errorLine = parsed.error;
      return;
    }
    if (run.sawReady || run.timedOut) return;
    run.sawReady = true;
    clearTimeout(run.readyTimer);
    void this.#afterReady(run, parsed.ready);
  }

  /** Main's control connection: track the project, recover after a supervised restart. */
  async #afterReady(run: Run, ready: EngineReady): Promise<void> {
    const epoch = this.#epoch;
    const live = () => epoch === this.#epoch && this.#run === run && !run.exited;
    let recovery: EngineRecovery | undefined;
    let recoveryError: EngineErrorInfo | undefined;
    try {
      const rpc = await this.#openControl(run, ready.port);
      const hello = helloProject(await this.#hello(rpc));
      if (!live()) return;
      if (run.recover && run.project !== null) {
        let applied = 0;
        let damaged = false;
        if (hello.recoveryPending) {
          try {
            const out = (await rpc.call(
              "project.recover",
              { _meta: { request_id: uuidv7() } },
              60_000,
            )) as { applied?: unknown; damaged?: unknown } | null;
            applied = typeof out?.applied === "number" ? out.applied : 0;
            damaged = out?.damaged === true;
          } catch (err) {
            recoveryError = {
              code: "recovery_failed",
              message: `automatic recovery failed: ${errorText(err)}`,
            };
          }
        }
        if (recoveryError === undefined) {
          recovery = { project: hello.path ?? run.project, applied, damaged };
        }
      }
      if (!live()) return;
      await this.#refreshProject(rpc);
      if (this.#windowFocused) this.#reportFocus();
      // The folders changed while this engine was starting: it gets the list of today.
      const roots = this.#o.allowRoots?.() ?? [];
      if (roots.length !== run.roots.length || roots.some((r, i) => r !== run.roots[i])) {
        await this.#sendAllowedRoots(rpc, roots);
      }
    } catch (err) {
      if (!live()) return;
      // The engine runs; only main's bookkeeping is missing (no project tracking or recovery).
      this.#log(`engine control connection failed: ${errorText(err)}`);
      if (run.recover) {
        recoveryError = {
          code: "recovery_failed",
          message: `automatic recovery failed: ${errorText(err)}`,
        };
      }
    }
    if (!live()) return;
    this.#portRetries = 0;
    if (ready.port !== this.#port) {
      // Not expected (main always passes a concrete port), but the CSP must name the real one.
      const previous = this.#port;
      this.#port = ready.port;
      this.#o.onPortChange?.(ready.port, previous);
    }
    if (recoveryError !== undefined) {
      this.#log(recoveryError.message);
      this.#lastError = recoveryError;
    }
    this.#recovery = recovery;
    this.#set("ready");
  }

  /** Open main's connection to `run`'s engine and follow its project events. */
  async #openControl(run: Run, port: number): Promise<EngineRpc> {
    const connect = this.#o.connectRpc ?? ((p: number, t: string) => connectEngineRpc(p, t));
    const rpc = await connect(port, run.token);
    if (this.#run !== run || run.exited || this.#stopping) {
      rpc.close();
      throw new Error("the engine went away");
    }
    this.#rpc = rpc;
    rpc.onNotification((method) => {
      if (PROJECT_EVENTS.includes(method)) void this.#refreshProject(rpc);
    });
    rpc.onClose(() => {
      if (this.#rpc !== rpc) return;
      this.#rpc = null;
      // Closed under a running engine (4001 `resync_required`): reconnect, or a later crash
      // would reopen a project the user has since closed.
      setTimeout(() => {
        if (this.#run !== run || run.exited || this.#stopping || this.#rpc !== null) return;
        this.#openControl(run, port)
          .then((again) => this.#refreshProject(again))
          .catch((err: unknown) => this.#log(`engine control reconnect failed: ${errorText(err)}`));
      }, CONTROL_RECONNECT_MS);
    });
    await rpc.call("rpc.subscribe", { events: PROJECT_EVENTS });
    return rpc;
  }

  #hello(rpc: EngineRpc): Promise<unknown> {
    return rpc.call("rpc.hello", {
      client: "cutweave-desktop-main",
      version: this.#o.clientVersion ?? "0.0.0",
    });
  }

  async #refreshProject(rpc: EngineRpc): Promise<void> {
    try {
      const hello = helloProject(await this.#hello(rpc));
      if (rpc !== this.#rpc) return;
      this.#project = hello.path;
      this.#recoveryPending = hello.recoveryPending;
    } catch {
      // The connection went away; the exit handler takes over.
    }
  }

  #closeRpc(): void {
    const rpc = this.#rpc;
    this.#rpc = null;
    rpc?.close();
  }

  // ----- exits -----------------------------------------------------------------------------

  #onExit(run: Run, code: number | null, signal: NodeJS.Signals | null): void {
    if (run.exited) return;
    run.exited = true;
    clearTimeout(run.readyTimer);
    if (this.#run !== run) return;
    this.#run = null;
    this.#closeRpc();
    if (this.#stopping) {
      this.#set("stopped");
      const waiters = this.#stopWaiters;
      this.#stopWaiters = [];
      for (const done of waiters) done();
      return;
    }
    const info = this.#describeExit(run, code, signal);
    // Exit 12: the port was taken between the pick and the bind; take another one, start again.
    if (!run.sawReady && code === EXIT.portInUse) {
      if (this.#portRetries < (this.#o.maxPortRetries ?? MAX_PORT_RETRIES)) {
        this.#portRetries++;
        const epoch = ++this.#epoch;
        void this.#changePort(epoch).then((ok) => {
          if (ok && epoch === this.#epoch) this.#spawn(run.restart);
        });
        return;
      }
      this.#fail(info);
      return;
    }
    if (!run.sawReady && !run.timedOut && code !== null && FATAL_EXITS.has(code)) {
      this.#fail(info);
      return;
    }
    this.#onCrash(info);
  }

  #describeExit(run: Run, code: number | null, signal: NodeJS.Signals | null): EngineErrorInfo {
    const how =
      code !== null
        ? describeExitCode(code)
        : signal !== null
          ? `signal ${signal}`
          : "no exit code";
    const base = {
      ...(code === null ? {} : { exitCode: code }),
      ...(signal === null ? {} : { signal }),
    };
    if (run.timedOut) {
      const s = (this.#o.readyTimeoutMs ?? READY_TIMEOUT_MS) / 1000;
      return { code: "ready_timeout", message: `the engine was not ready within ${s} s`, ...base };
    }
    if (run.errorLine !== null) {
      const { code: lineCode, message, holder } = run.errorLine;
      return { code: lineCode, message, ...base, ...(holder === undefined ? {} : { holder }) };
    }
    const pid = run.child.pid;
    const panic =
      pid === undefined
        ? null
        : (this.#o.readCrashMarker ?? ((p: number) => this.#readCrashMarker(p)))(pid);
    const message = run.sawReady
      ? `the engine stopped unexpectedly (${how})`
      : `the engine exited before it was ready (${how})`;
    return {
      code: run.sawReady ? "crashed" : "start_failed",
      message: panic ? `${message}: ${panic.message}` : message,
      ...base,
      ...(panic ? { panic } : {}),
    };
  }

  #readCrashMarker(pid: number): EngineErrorInfo["panic"] | null {
    const env = { ...process.env, ...this.#o.env };
    const args = this.#o.extraArgs ?? [];
    const i = args.indexOf("--log-dir");
    const dir = i >= 0 ? args[i + 1] : defaultEngineLogDir(env);
    return dir ? readCrashMarker(dir, pid) : null;
  }

  #onCrash(info: EngineErrorInfo): void {
    const now = (this.#o.now ?? Date.now)();
    const window = this.#o.crashWindowMs ?? CRASH_WINDOW_MS;
    const backoff = this.#o.backoffMs ?? RESTART_BACKOFF_MS;
    this.#crashes = this.#crashes.filter((t) => now - t < window);
    this.#crashes.push(now);
    this.#lastError = info;
    this.#log(`${info.message} (${this.#crashes.length} in ${window / 1000} s)`);
    if (this.#crashes.length > backoff.length) {
      this.#fail(info, `The engine stopped ${this.#crashes.length} times in ${window / 1000} s.`);
      return;
    }
    const delay = backoff[this.#crashes.length - 1] ?? 0;
    const epoch = ++this.#epoch;
    this.#set("crashed", { restartInMs: delay });
    this.#restartTimer = setTimeout(() => {
      if (epoch === this.#epoch) void this.#launch("restart");
    }, delay);
  }

  #fail(info: EngineErrorInfo, summary?: string): void {
    this.#lastError = info;
    this.#epoch++;
    clearTimeout(this.#restartTimer);
    this.#log(`engine failed: ${info.message}`);
    this.#set("failed", {
      message: summary ? `${summary} Last error: ${info.message}` : info.message,
    });
  }

  // ----- status ----------------------------------------------------------------------------

  #set(state: EngineState, extra: { message?: string; restartInMs?: number } = {}): void {
    const lastError = this.#lastError;
    const message =
      extra.message ??
      (state === "crashed" || state === "restarting" ? lastError?.message : undefined);
    this.#status = {
      state,
      ...(state === "ready" ? { port: this.#port } : {}),
      ...(message === undefined ? {} : { message }),
      ...(this.#generation > 0 ? { generation: this.#generation } : {}),
      ...(lastError === undefined ? {} : { lastError }),
      ...(state === "ready" && this.#recovery !== undefined ? { recovery: this.#recovery } : {}),
      ...(extra.restartInMs === undefined ? {} : { restartInMs: extra.restartInMs }),
    };
    for (const listener of [...this.#listeners]) listener(this.#status);
  }

  #log(message: string): void {
    (this.#o.log ?? ((m: string) => console.warn(`[engine] ${m}`)))(message);
  }
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
