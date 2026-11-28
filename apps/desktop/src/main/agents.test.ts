// SPDX-License-Identifier: GPL-3.0-or-later
import path from "node:path";
import { describe, expect, it } from "vitest";
import { MAX_AGENT_FOLDERS } from "../../../../packages/preferences/src/index.ts";
import { AGENT_CLIENTS } from "../shared/api.ts";
import {
  addFolder,
  clientProfile,
  clientSetups,
  engineAllowRoots,
  mcpArgs,
  resolveChosenFolder,
  resolveCliPath,
  shellQuote,
} from "./agents.ts";

describe("resolveCliPath", () => {
  const base = {
    env: {},
    isPackaged: false,
    resourcesPath: "/app/resources",
    repoRoot: "/repo",
    platform: "linux" as const,
  };

  it("is the bundled engine binary when packaged, never an environment override", () => {
    const bundled = path.join("/app/resources", "engine", "cutweave");
    expect(
      resolveCliPath({
        ...base,
        isPackaged: true,
        env: { CUTWEAVE_CLI_BIN: "/evil/cutweave" },
        exists: () => true,
      }),
    ).toBe(bundled);
    expect(resolveCliPath({ ...base, isPackaged: true, exists: () => false })).toBeNull();
    expect(
      resolveCliPath({
        ...base,
        isPackaged: true,
        platform: "win32",
        exists: (f) => f.endsWith("cutweave.exe"),
      }),
    ).toBe(path.join("/app/resources", "engine", "cutweave.exe"));
  });

  it("takes an absolute CUTWEAVE_CLI_BIN, then target/debug in development", () => {
    expect(
      resolveCliPath({ ...base, env: { CUTWEAVE_CLI_BIN: "/t/cutweave" }, exists: () => true }),
    ).toBe("/t/cutweave");
    expect(
      resolveCliPath({ ...base, env: { CUTWEAVE_CLI_BIN: "rel/cutweave" }, exists: () => true }),
    ).toBe(path.join("/repo", "target", "debug", "cutweave"));
    expect(resolveCliPath({ ...base, exists: () => false })).toBeNull();
  });
});

describe("the client setups", () => {
  const forbidden = /--pid|--port|token|\b\d{4,5}\b/i;

  it("run the bundled CLI's absolute path with mcp --attach and no pid, port or token", () => {
    const cli = "C:\\Program Files\\Cutweave\\resources\\engine\\cutweave.exe";
    const setups = clientSetups(cli, "standard", "win32");
    expect(setups.map((s) => s.client)).toEqual([...AGENT_CLIENTS]);
    for (const { client, text } of setups) {
      expect(text, client).not.toMatch(forbidden);
      expect(text, client).toContain("--attach");
    }
    const byClient = Object.fromEntries(setups.map((s) => [s.client, s.text]));
    // Claude Code defers tool definitions: its setup gets the described list (WP-2I.05).
    expect(byClient["claude-code"]).toBe(
      `claude mcp add cutweave -- "${cli}" mcp --attach --profile described`,
    );
    expect(JSON.parse(byClient["claude-desktop"] ?? "")).toEqual({
      mcpServers: { cutweave: { command: cli, args: ["mcp", "--attach"] } },
    });
    expect(byClient.cursor).toBe(byClient["claude-desktop"]);
    expect(byClient.codex).toBe(
      `[mcp_servers.cutweave]\ncommand = '${cli}'\nargs = ["mcp", "--attach"]`,
    );
    expect(byClient.stdio).toBe(`"${cli}" mcp --attach`);
  });

  it("name the tool set when it is not the standard one", () => {
    expect(mcpArgs("standard")).toEqual(["mcp", "--attach"]);
    const minimal = clientSetups("/opt/cutweave/cutweave", "minimal", "linux");
    expect(minimal[0]?.text).toBe(
      "claude mcp add cutweave -- /opt/cutweave/cutweave mcp --attach --profile minimal",
    );
    const full = clientSetups("/opt/cutweave/cutweave", "full", "linux");
    expect(full.find((s) => s.client === "cursor")?.text).toContain('"--profile",\n');
  });

  it("give each client the list that suits it, unless another is chosen", () => {
    for (const client of AGENT_CLIENTS) {
      expect(clientProfile(client, "standard"), client).toBe(
        client === "claude-code" ? "described" : "standard",
      );
      for (const chosen of ["minimal", "described", "full"] as const) {
        expect(clientProfile(client, chosen), client).toBe(chosen);
      }
    }
    // Described, chosen: every setup carries it (Copilot CLI through "Other (stdio)").
    const described = clientSetups("/opt/cutweave/cutweave", "described", "linux");
    for (const { client, text } of described) {
      expect(text, client).toContain("described");
    }
    expect(described.find((s) => s.client === "stdio")?.text).toBe(
      "/opt/cutweave/cutweave mcp --attach --profile described",
    );
    // Minimal, chosen: Claude Code gets minimal too.
    const minimal = clientSetups("/opt/cutweave/cutweave", "minimal", "linux");
    expect(minimal.find((s) => s.client === "claude-code")?.text).toContain("--profile minimal");
  });

  it("quote a path for the shell it is pasted into", () => {
    expect(shellQuote("/opt/cut weave/cutweave", "linux")).toBe("'/opt/cut weave/cutweave'");
    expect(shellQuote("/opt/it's/cutweave", "darwin")).toBe("'/opt/it'\\''s/cutweave'");
    expect(shellQuote("C:\\Tools\\cutweave.exe", "win32")).toBe("C:\\Tools\\cutweave.exe");
    expect(shellQuote("C:\\My Tools\\cutweave.exe", "win32")).toBe('"C:\\My Tools\\cutweave.exe"');
  });
});

describe("the folders", () => {
  const fs = (real: Record<string, string>, dirs: Set<string>) => ({
    realpath: (p: string) => {
      const r = real[p];
      if (r === undefined) throw new Error("ENOENT");
      return r;
    },
    isDirectory: (p: string) => dirs.has(p),
  });

  it("stores the folder the picker returned resolved, and only an existing folder", () => {
    const disk = fs({ "/link": "/real/footage", "/file": "/file" }, new Set(["/real/footage"]));
    expect(resolveChosenFolder("/link", disk)).toEqual({ ok: true, folder: "/real/footage" });
    expect(resolveChosenFolder("/file", disk)).toEqual({ ok: false });
    expect(resolveChosenFolder("/missing", disk)).toEqual({ ok: false });
    for (const bad of ["", "relative", "/a\u0000b", 42, null, "/x".repeat(3000)]) {
      expect(resolveChosenFolder(bad, disk), String(bad)).toEqual({ ok: false });
    }
  });

  it("adds a folder once, up to the limit", () => {
    expect(addFolder(["/a"], "/b", "linux")).toEqual({
      status: "added",
      folder: "/b",
      folders: ["/a", "/b"],
    });
    expect(addFolder(["C:\\Footage"], "c:\\footage", "win32")).toEqual({
      status: "exists",
      folder: "c:\\footage",
    });
    const full = Array.from({ length: MAX_AGENT_FOLDERS }, (_, i) => `/f${i}`);
    expect(addFolder(full, "/one-more", "linux")).toEqual({ status: "full" });
  });

  it("gives the engine the development folders, then the stored ones that still exist", () => {
    const warnings: string[] = [];
    const roots = engineAllowRoots(["/dev"], ["/a", "/dev", "/gone", "/b"], {
      platform: "linux",
      isDirectory: (p) => p !== "/gone",
      log: (m) => warnings.push(m),
    });
    expect(roots).toEqual(["/dev", "/a", "/b"]);
    expect(warnings).toEqual(["skipping a folder agents may use: /gone no longer exists"]);
  });
});
