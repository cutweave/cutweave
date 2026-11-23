// SPDX-License-Identifier: GPL-3.0-or-later
// The supervisor's side of the engine launch protocol (docs/reference/launch-protocol.md,
// WP-1I.04): exit codes, the stdout JSON lines (ready / error) and the crash marker.

import { readFileSync } from "node:fs";
import path from "node:path";
import type { EngineErrorInfo } from "../../shared/api.ts";

/** Engine exit codes (launch protocol "Exit codes"). */
export const EXIT = {
  ok: 0,
  failure: 1,
  usage: 2,
  projectLocked: 10,
  gpuInitFailed: 11,
  portInUse: 12,
  badHandshake: 13,
} as const;

/** Engine exit code when it cannot bind the port main chose. */
export const EXIT_PORT_IN_USE = EXIT.portInUse;

export interface EngineReady {
  readonly port: number;
  readonly pid: number;
  readonly version: string;
}

/** `{"event":"error","code","exit_code","message","holder"?}`, printed before a failed start exits. */
export interface EngineErrorLine {
  readonly code: string;
  readonly exitCode: number;
  readonly message: string;
  readonly holder?: { readonly pid: number; readonly host: string; readonly started: number };
}

export type EngineLine =
  | { readonly event: "ready"; readonly ready: EngineReady }
  | { readonly event: "error"; readonly error: EngineErrorLine };

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** Parse one stdout line; null for anything that is not a well-formed ready or error event. */
export function parseEngineLine(line: string): EngineLine | null {
  let data: unknown;
  try {
    data = JSON.parse(line);
  } catch {
    return null;
  }
  if (!isObject(data)) return null;
  if (data.event === "ready") {
    if (typeof data.port !== "number" || typeof data.pid !== "number") return null;
    const version = typeof data.version === "string" ? data.version : "";
    return { event: "ready", ready: { port: data.port, pid: data.pid, version } };
  }
  if (data.event === "error") {
    if (typeof data.code !== "string" || typeof data.exit_code !== "number") return null;
    const message = typeof data.message === "string" ? data.message : data.code;
    const h = data.holder;
    const holder =
      isObject(h) &&
      typeof h.pid === "number" &&
      typeof h.host === "string" &&
      typeof h.started === "number"
        ? { pid: h.pid, host: h.host, started: h.started }
        : undefined;
    return {
      event: "error",
      error: {
        code: data.code,
        exitCode: data.exit_code,
        message,
        ...(holder === undefined ? {} : { holder }),
      },
    };
  }
  return null;
}

/** Parse the readiness line `{"event":"ready","port":P,"pid":N,"version":"…"}`; null otherwise. */
export function parseReadyLine(line: string): EngineReady | null {
  const parsed = parseEngineLine(line);
  return parsed?.event === "ready" ? parsed.ready : null;
}

/** `~/.cutweave/logs` as the engine resolves it (`USERPROFILE`, then `HOME`) for `env`. */
export function defaultEngineLogDir(
  env: Readonly<Record<string, string | undefined>>,
): string | null {
  const home = env.USERPROFILE || env.HOME;
  return home ? path.join(home, ".cutweave", "logs") : null;
}

/**
 * The crash marker a panicking engine leaves next to its log (`crash-<pid>.json`); a clean exit
 * removes it, so one that survives the process means the engine crashed. Null when absent or
 * unreadable.
 */
export function readCrashMarker(logDir: string, pid: number): EngineErrorInfo["panic"] | null {
  let data: unknown;
  try {
    data = JSON.parse(readFileSync(path.join(logDir, `crash-${pid}.json`), "utf8"));
  } catch {
    return null;
  }
  if (!isObject(data) || typeof data.message !== "string") return null;
  return typeof data.location === "string"
    ? { message: data.message, location: data.location }
    : { message: data.message };
}
