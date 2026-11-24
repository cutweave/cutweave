// SPDX-License-Identifier: GPL-3.0-or-later
// Settings > Agents and MCP, main's half (WP-1J.05; arch §14, §16, §18, ADR-0016):
//
// - "Folders agents can use" (`agents.allowed_folders`, this computer's, only main writes it):
//   a folder is added with main's own folder picker and stored resolved, so the renderer never
//   names a path agents may then use. Every engine start and supervised restart gets the folders
//   that still exist as `--allow-root` (a missing one is skipped with a warning in the main log);
//   a change goes to the running engine at once with `project.set_allowed_roots` (UI-only).
// - "Connect a client": the setup each agent host needs to run the bundled CLI as a stdio MCP
//   server, `<cli> mcp --attach`, with no pid (the engine of the window used last, WP-1J.04),
//   no port and no token (the attach proxy reads the run-file). Main builds the text and puts it
//   on the clipboard itself; the renderer only names which client.

import { existsSync, realpathSync, statSync } from "node:fs";
import path from "node:path";
import {
  type ClientProfile,
  MAX_AGENT_FOLDERS,
} from "../../../../packages/preferences/src/index.ts";
import { AGENT_CLIENTS, type AgentClient, type ClientSetup } from "../shared/api.ts";

/** The longest folder path main stores. */
const MAX_PATH_LENGTH = 4096;

export interface CliPathOptions {
  env: Readonly<Record<string, string | undefined>>;
  isPackaged: boolean;
  /** `process.resourcesPath` of the packaged app (the CLI is `resources/engine/cutweave`). */
  resourcesPath: string;
  /** Repository root, for dev builds under `target/debug/`. */
  repoRoot: string;
  platform: NodeJS.Platform;
  exists?: (file: string) => boolean;
}

/**
 * The bundled `cutweave` CLI agents run (an absolute path), or null when this build has none.
 * Packaged: `resources/engine/cutweave` (the engine binary itself). Unpackaged: an absolute
 * `CUTWEAVE_CLI_BIN` (tests), then `target/debug/cutweave`.
 */
export function resolveCliPath(options: CliPathOptions): string | null {
  const exists = options.exists ?? existsSync;
  const exe = options.platform === "win32" ? "cutweave.exe" : "cutweave";
  if (options.isPackaged) {
    const bundled = path.join(options.resourcesPath, "engine", exe);
    return exists(bundled) ? bundled : null;
  }
  const override = options.env.CUTWEAVE_CLI_BIN;
  if (override && path.isAbsolute(override) && exists(override)) return override;
  const dev = path.join(options.repoRoot, "target", "debug", exe);
  return exists(dev) ? dev : null;
}

// ---- the folders ------------------------------------------------------------------------------

/** Whether two stored folders are the same (case-insensitively on Windows and macOS). */
export function sameFolder(a: string, b: string, platform: NodeJS.Platform): boolean {
  return platform === "linux" ? a === b : a.toLowerCase() === b.toLowerCase();
}

export type FolderCheck = { ok: true; folder: string } | { ok: false };

/**
 * The folder the OS picker returned, resolved (symlinks and junctions followed) and checked: an
 * absolute, bounded, NUL-free path of an existing folder. Anything else is refused.
 */
export function resolveChosenFolder(
  chosen: unknown,
  fs: { realpath(p: string): string; isDirectory(p: string): boolean } = {
    realpath: (p) => realpathSync.native(p),
    isDirectory: (p) => statSync(p).isDirectory(),
  },
): FolderCheck {
  if (
    typeof chosen !== "string" ||
    chosen.length === 0 ||
    chosen.length > MAX_PATH_LENGTH ||
    chosen.includes("\u0000") ||
    !path.isAbsolute(chosen)
  ) {
    return { ok: false };
  }
  try {
    const real = fs.realpath(chosen);
    if (!path.isAbsolute(real) || real.length > MAX_PATH_LENGTH || !fs.isDirectory(real)) {
      return { ok: false };
    }
    return { ok: true, folder: real };
  } catch {
    return { ok: false };
  }
}

export type AddFolderOutcome =
  | { status: "added"; folder: string; folders: string[] }
  | { status: "exists"; folder: string }
  | { status: "full" }
  | { status: "invalid" };

/** `folder` (resolved) added to `folders`, unless it is there already or the list is full. */
export function addFolder(
  folders: readonly string[],
  folder: string,
  platform: NodeJS.Platform,
): AddFolderOutcome {
  if (folders.some((f) => sameFolder(f, folder, platform))) return { status: "exists", folder };
  if (folders.length >= MAX_AGENT_FOLDERS) return { status: "full" };
  return { status: "added", folder, folders: [...folders, folder] };
}

/**
 * The folders an engine gets (`--allow-root` at start, `project.set_allowed_roots` on change):
 * the development folders first (unpackaged runs only), then the stored ones, each once; a folder
 * that no longer exists is skipped with a warning (it stays stored: a drive may come back).
 */
export function engineAllowRoots(
  devRoots: readonly string[],
  stored: readonly string[],
  options: {
    platform: NodeJS.Platform;
    isDirectory?: (p: string) => boolean;
    log?: (message: string) => void;
  },
): string[] {
  const isDirectory =
    options.isDirectory ??
    ((p: string) => {
      try {
        return statSync(p).isDirectory();
      } catch {
        return false;
      }
    });
  const out: string[] = [];
  for (const folder of [...devRoots, ...stored]) {
    if (out.some((f) => sameFolder(f, folder, options.platform))) continue;
    if (!isDirectory(folder)) {
      options.log?.(`skipping a folder agents may use: ${folder} no longer exists`);
      continue;
    }
    out.push(folder);
  }
  return out;
}

// ---- the client setups ------------------------------------------------------------------------

/** `cutweave mcp` arguments for `profile` (standard is the default and needs no flag). */
export function mcpArgs(profile: ClientProfile): string[] {
  return profile === "standard" ? ["mcp", "--attach"] : ["mcp", "--attach", "--profile", profile];
}

/** One argument for the shell the person pastes into: quoted when it needs to be. */
export function shellQuote(arg: string, platform: NodeJS.Platform): string {
  if (platform === "win32") {
    return /^[A-Za-z0-9_./:\\-]+$/.test(arg) ? arg : `"${arg.replaceAll('"', '\\"')}"`;
  }
  return /^[A-Za-z0-9_./:-]+$/.test(arg) ? arg : `'${arg.replaceAll("'", "'\\''")}'`;
}

/** A TOML string: a literal string when it can be, else a basic string with escapes. */
function tomlString(value: string): string {
  const control = [...value].some((c) => c.charCodeAt(0) < 0x20);
  if (!value.includes("'") && !control) return `'${value}'`;
  return JSON.stringify(value);
}

/** The JSON `mcpServers` entry Claude Desktop and Cursor read. */
function mcpServersJson(cli: string, args: readonly string[]): string {
  return JSON.stringify({ mcpServers: { cutweave: { command: cli, args } } }, null, 2);
}

/**
 * The text of each client's setup, running `cli` with `profile`. Never a pid, a port or a token:
 * `cutweave mcp --attach` finds the engine of the window used last through its run-file.
 */
export function clientSetups(
  cli: string,
  profile: ClientProfile,
  platform: NodeJS.Platform,
): ClientSetup[] {
  const args = mcpArgs(profile);
  const commandLine = [cli, ...args].map((a) => shellQuote(a, platform)).join(" ");
  const text: Record<AgentClient, string> = {
    "claude-code": `claude mcp add cutweave -- ${commandLine}`,
    "claude-desktop": mcpServersJson(cli, args),
    cursor: mcpServersJson(cli, args),
    codex: [
      "[mcp_servers.cutweave]",
      `command = ${tomlString(cli)}`,
      `args = ${JSON.stringify(args).replaceAll(",", ", ")}`,
    ].join("\n"),
    stdio: commandLine,
  };
  return AGENT_CLIENTS.map((client) => ({ client, text: text[client] }));
}
