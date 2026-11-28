// SPDX-License-Identifier: GPL-3.0-or-later
// F20 Agent folders and connections (docs/user-flows.md, WP-1J.05), as a person does it: in
// Settings > Agents and MCP add a folder with the OS folder picker (the one stub: the picker is
// the OS's); an agent attached to the app with the real `cutweave exec` imports a file from it and
// its card appears in the bin; a notice names the agent; an MCP agent started from the copied
// Claude Code setup (the real `cutweave mcp --attach`) is listed under Connected agents while it is
// connected; the folder is removed and the agents' next imports are refused with a message naming
// the setting; the list survives a relaunch, and the relaunched engine lets agents use the folder.
import { type ChildProcessByStdio, execFile, spawn } from "node:child_process";
import { copyFileSync, mkdtempSync, readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";
import { promisify } from "node:util";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  cliBin,
  closeSettings,
  createProject,
  hasCli,
  hasRealEngine,
  launchFlowApp,
  mediaCard,
  openSettingsPage,
  pickFiles,
  profileDir,
  removeDir,
  snapshot,
  toast,
  workspace,
} from "./helpers.ts";

const run = promisify(execFile);
const SETTING = "Settings > Agents and MCP";

const runDir = (home: string): string => path.join(home, ".cutweave", "run");

/** The environment an agent on this machine has: the app's HOME, so the run-files are shared. */
function agentEnv(home: string): NodeJS.ProcessEnv {
  // Dev stub media in the CLI's own environment would make it spawn an engine of its own.
  const { CUTWEAVE_DEV_STUB_MEDIA: _stub, ...inherited } = process.env;
  return { ...inherited, HOME: home, USERPROFILE: home };
}

/**
 * `cutweave exec <step>` as a script runs it: attached to the app's engine with no pid (the
 * window used last). The exit code and the JSON it printed (stdout, or the error on stderr).
 */
async function exec(
  home: string,
  step: { cmd: string; args: Record<string, unknown> },
): Promise<{ code: number; json: Record<string, unknown> }> {
  try {
    const { stdout } = await run(
      cliBin,
      ["exec", "--run-dir", runDir(home), JSON.stringify(step)],
      { env: agentEnv(home), timeout: 30_000 },
    );
    return { code: 0, json: JSON.parse(stdout) as Record<string, unknown> };
  } catch (error) {
    const failed = error as { code?: unknown; stderr?: string };
    return {
      code: typeof failed.code === "number" ? failed.code : -1,
      json: JSON.parse(failed.stderr || "{}") as Record<string, unknown>,
    };
  }
}

const importStep = (file: string) => ({ cmd: "media.import", args: { paths: [file] } });

/** The arguments of a pasted command line (double or single quotes group). */
function splitCommandLine(line: string): string[] {
  const out: string[] = [];
  for (const m of line.matchAll(/"((?:[^"\\]|\\.)*)"|'([^']*)'|(\S+)/g)) {
    out.push(m[1]?.replaceAll('\\"', '"') ?? m[2] ?? m[3] ?? "");
  }
  return out;
}

interface ToolResult {
  isError?: boolean;
  structuredContent?: Record<string, unknown> & { error?: { code?: string; message?: string } };
}

/** An MCP host's side of a stdio server: newline-delimited JSON-RPC (MCP 2025-11-25). */
class McpAgent {
  readonly #child: ChildProcessByStdio<Writable, Readable, null>;
  readonly #waiting = new Map<number, (message: Record<string, unknown>) => void>();
  #next = 1;

  private constructor(child: ChildProcessByStdio<Writable, Readable, null>) {
    this.#child = child;
    createInterface({ input: child.stdout }).on("line", (line) => {
      const message = JSON.parse(line) as Record<string, unknown>;
      const id = message.id;
      if (typeof id === "number" && ("result" in message || "error" in message)) {
        this.#waiting.get(id)?.(message);
        this.#waiting.delete(id);
      }
    });
  }

  /** Starts `command args…` as the agent host would, and runs the MCP handshake. */
  static async start(command: string, args: string[], home: string): Promise<McpAgent> {
    const child = spawn(command, args, {
      env: agentEnv(home),
      stdio: ["pipe", "pipe", "ignore"],
      windowsHide: true,
    });
    const agent = new McpAgent(child);
    const hello = await agent.#request("initialize", {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: "f20-agent", version: "1.0.0" },
    });
    expect(hello.error).toBeUndefined();
    agent.#send({ jsonrpc: "2.0", method: "notifications/initialized" });
    return agent;
  }

  #send(message: Record<string, unknown>): void {
    this.#child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  #request(method: string, params: unknown): Promise<Record<string, unknown>> {
    const id = this.#next++;
    const answered = new Promise<Record<string, unknown>>((resolve) =>
      this.#waiting.set(id, resolve),
    );
    this.#send({ jsonrpc: "2.0", id, method, params });
    return answered;
  }

  async tool(name: string, args: Record<string, unknown>): Promise<ToolResult> {
    const reply = await this.#request("tools/call", { name, arguments: args });
    expect(reply.error, JSON.stringify(reply)).toBeUndefined();
    return reply.result as ToolResult;
  }

  /** The host quits the server: its stdin closes and it exits. */
  async stop(): Promise<void> {
    const exited = new Promise<void>((resolve) => {
      if (this.#child.exitCode !== null) resolve();
      else this.#child.once("exit", () => resolve());
    });
    this.#child.stdin.end();
    await exited;
  }

  kill(): void {
    if (this.#child.exitCode === null) this.#child.kill();
  }
}

/** The agent tokens of the app's run-file (nothing copied may contain them). */
function agentTokens(home: string): string[] {
  return readdirSync(runDir(home))
    .filter((name) => name.endsWith(".json"))
    .flatMap((name) => {
      const file = JSON.parse(readFileSync(path.join(runDir(home), name), "utf8")) as {
        agent_tokens?: { token: string }[];
      };
      return (file.agent_tokens ?? []).map((t) => t.token);
    });
}

const folderList = (settings: Locator): Locator =>
  settings.getByRole("list", { name: "Folders agents can use" });
const connectedList = (settings: Locator): Locator =>
  settings.getByRole("list", { name: "Connected agents" });

/** "Add folder…" with the OS folder picker answering `folder`; resolves to the stored folder. */
async function addFolder(app: ElectronApplication, settings: Locator, folder: string) {
  await pickFiles(app, [folder]);
  await settings.getByRole("button", { name: "Add folder…" }).click();
  // Main stores it resolved: a temp folder may come back under its long name.
  const added = settings.getByText(/^Agents can use .+ now\.$/);
  await expect(added).toBeVisible();
  const stored = ((await added.textContent()) ?? "")
    .replace(/^Agents can use /, "")
    .replace(/ now\.$/, "");
  expect(path.basename(stored)).toBe(path.basename(folder));
  return stored;
}

/** Closes Settings with Done and waits for the toast `title` naming `name`. */
async function expectNotice(page: Page, title: string, name: string): Promise<void> {
  await expect(toast(page, title).filter({ hasText: name })).toBeVisible();
}

test.describe("F20 agent folders and connections", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  test.skip(!hasCli, "the cutweave CLI is not built (cargo build -p cutweave-cli)");

  test("add a folder, an agent imports from it, remove it, copy a setup, relaunch", async () => {
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    // The footage folder lies outside the project and every folder the app was started with.
    const footage = mkdtempSync(path.join(tmpdir(), "cutweave-flow-footage-"));
    for (const name of ["interview.mp4", "logo.png", "music.wav", "broll-city.mp4"] as const) {
      copyFileSync(work.file(name), path.join(footage, name));
    }
    const inFootage = (name: string) => path.join(footage, name);
    let mcp: McpAgent | null = null;
    const first = await launchFlowApp(work, home);
    try {
      const { app, page } = first;
      await createProject(app, page, "Agents", work.dir);

      // Before the person allows it, an agent cannot use the folder, and is told where to ask.
      const before = await exec(home, importStep(inFootage("interview.mp4")));
      expect(before.code).toBe(1);
      expect(before.json.code).toBe("invalid");
      expect(String(before.json.message)).toContain(SETTING);

      // Settings > Agents and MCP: agents can connect; the project folder is always listed.
      let settings = await openSettingsPage(page, app, "Agents and MCP");
      await expect(settings.getByText("Agents can connect to this window")).toBeVisible();
      await expect(folderList(settings)).toContainText("(the open project, always allowed)");
      await expect(folderList(settings).getByRole("button")).toHaveCount(0);
      const allowed = await addFolder(app, settings, footage);
      await expect(
        folderList(settings).getByRole("button", { name: `Remove ${allowed}` }),
      ).toBeVisible();

      // Copy the Claude Code setup: the bundled CLI's path, and no pid, port or token.
      const setup = settings.getByLabel("Setup for Claude Code");
      await expect(setup).toContainText("claude mcp add cutweave --");
      await settings.getByRole("button", { name: "Copy" }).click();
      await expect(settings.getByText("Copied the Claude Code setup.")).toBeVisible();
      const copied = await app.evaluate(({ clipboard }) => clipboard.readText());
      expect(copied).toBe((await setup.textContent())?.trim());
      expect(copied.startsWith("claude mcp add cutweave -- ")).toBe(true);
      const [command, ...args] = splitCommandLine(
        copied.replace("claude mcp add cutweave -- ", ""),
      );
      expect(path.resolve(command ?? "")).toBe(path.resolve(cliBin));
      // Claude Code loads a tool's definition when it needs it, so its setup asks for the
      // list with parameter descriptions (WP-2I.05).
      expect(args).toEqual(["mcp", "--attach", "--profile", "described"]);
      expect(copied).not.toMatch(/--pid|--port|token/i);
      const port = await app.evaluate(() => globalThis.__cutweaveTest?.enginePort() ?? null);
      if (typeof port === "number") expect(copied).not.toContain(String(port));
      for (const token of agentTokens(home)) expect(copied).not.toContain(token);
      await snapshot(page, info, "agents page");
      await closeSettings(settings);

      // The CLI imports a file from the folder: its card appears, and a notice names the agent.
      const imported = await exec(home, importStep(inFootage("interview.mp4")));
      expect(imported.code, JSON.stringify(imported.json)).toBe(0);
      await expect(mediaCard(page, "interview.mp4")).toBeVisible();
      await expectNotice(page, "Agent connected", "Command line (agent:cli)");

      // An MCP agent started from the copied setup (the test's HOME is the app's, hence
      // --run-dir) stays connected: it imports, and Settings lists it while it is connected.
      mcp = await McpAgent.start(command ?? cliBin, [...args, "--run-dir", runDir(home)], home);
      const logo = await mcp.tool("media_import", { paths: [inFootage("logo.png")] });
      expect(logo.isError, JSON.stringify(logo)).not.toBe(true);
      await expect(mediaCard(page, "logo.png")).toBeVisible();
      await expectNotice(page, "Agent connected", "MCP client (agent:mcp)");
      settings = await openSettingsPage(page, app, "Agents and MCP");
      const mcpRow = connectedList(settings)
        .getByRole("listitem")
        .filter({ hasText: "Agent: mcp" });
      await expect(mcpRow).toBeVisible();
      await expect(mcpRow).toContainText("Last: media.import at");
      await snapshot(page, info, "connected agents");

      // The person removes the folder: the same open MCP session and the CLI are refused at once.
      await folderList(settings)
        .getByRole("button", { name: `Remove ${allowed}` })
        .click();
      await expect(settings.getByText(`Agents can no longer use ${allowed}.`)).toBeVisible();
      const refused = await mcp.tool("media_import", { paths: [inFootage("music.wav")] });
      expect(refused.isError).toBe(true);
      expect(refused.structuredContent?.error?.code).toBe("invalid");
      expect(refused.structuredContent?.error?.message).toContain(SETTING);
      const cliRefused = await exec(home, importStep(inFootage("music.wav")));
      expect(cliRefused.code).toBe(1);
      expect(String(cliRefused.json.message)).toContain(SETTING);

      // The agent leaves: it drops off the list, and a notice says so.
      await mcp.stop();
      mcp = null;
      await expect(settings.getByText("No agent is connected.")).toBeVisible();
      await closeSettings(settings);
      await expectNotice(page, "Agent disconnected", "MCP client (agent:mcp)");

      // Allowed again, then a relaunch: the list is kept and the new engine starts with it.
      settings = await openSettingsPage(page, app, "Agents and MCP");
      await addFolder(app, settings, footage);
      await closeSettings(settings);
    } finally {
      mcp?.kill();
      await first.close();
    }

    const second = await launchFlowApp(work, home);
    try {
      const { app, page } = second;
      const settings = await openSettingsPage(page, app, "Agents and MCP");
      await expect(folderList(settings)).toContainText(path.basename(footage));
      await closeSettings(settings);
      const again = await exec(home, importStep(inFootage("broll-city.mp4")));
      expect(again.code, JSON.stringify(again.json)).toBe(0);
      await expect(mediaCard(page, "broll-city.mp4")).toBeVisible();
      await snapshot(page, info, "after relaunch");
    } finally {
      await second.close();
      work.remove();
      removeDir(footage);
      removeDir(home);
    }
  });
});
