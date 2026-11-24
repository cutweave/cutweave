// SPDX-License-Identifier: GPL-3.0-or-later
// Run-level check of the desktop e2e suites (ADR-0018): when the run ends, no Electron, engine,
// CLI or fake-engine process this checkout started may still be running. Playwright's global
// setup notes what was already running (a dev app of this checkout, say); the teardown it
// returns waits briefly for the rest to exit, then kills and reports any that did not. Other
// checkouts' processes (other worktrees running their own suites) are never touched.
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import path from "node:path";
import { desktopDir, realEngine } from "./fixtures.ts";
import { cliBin } from "./flows/helpers.ts";

interface Proc {
  pid: number;
  name: string;
  exe: string;
  cmd: string;
}

const win = process.platform === "win32";
const norm = (p: string): string => {
  const resolved = path.resolve(p);
  return win ? resolved.toLowerCase() : resolved;
};

/** The processes named `names` (executable file names), with executable and command line. */
function processes(names: readonly string[]): Proc[] {
  if (win) {
    const filter = names.map((n) => `Name='${n.replace(/'/g, "''")}'`).join(" OR ");
    const script = [
      "[Console]::OutputEncoding = [Text.Encoding]::UTF8",
      `Get-CimInstance Win32_Process -Filter "${filter}" | ForEach-Object {`,
      "  $cmd = [string]$_.CommandLine -replace '[\\x00-\\x1f]', ' '",
      '  "$($_.ProcessId)`t$($_.Name)`t$($_.ExecutablePath)`t$cmd"',
      "}",
    ].join("\n");
    const out = spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], {
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
    });
    if (out.status !== 0) throw new Error(`listing processes failed: ${out.stderr}`);
    return out.stdout
      .split(/\r?\n/)
      .filter((line) => line.trim() !== "")
      .map((line) => {
        const [pid = "", name = "", exe = "", cmd = ""] = line.split("\t");
        return { pid: Number(pid), name, exe, cmd };
      });
  }
  const out = spawnSync("ps", ["-eo", "pid=,comm=,args="], { encoding: "utf8" });
  return out.stdout
    .split("\n")
    .map((line) => line.trim().match(/^(\d+)\s+(\S+)\s+(.*)$/))
    .filter((m): m is RegExpMatchArray => m !== null)
    .map((m) => ({
      pid: Number(m[1]),
      name: m[2] ?? "",
      exe: (m[3] ?? "").split(" ")[0] ?? "",
      cmd: m[3] ?? "",
    }))
    .filter((p) => names.some((n) => p.name === n || path.basename(p.exe) === n));
}

/**
 * The processes this checkout's suites start: its Electron, engine, CLI and ring fixture
 * binaries, and node running its test-support scripts (the fake engine).
 */
function ourProcesses(): () => Proc[] {
  const require = createRequire(import.meta.url);
  const electron = require("electron") as unknown as string;
  const ringFixture = path.join(
    desktopDir,
    "..",
    "..",
    "target",
    "debug",
    "examples",
    "ring_fixture",
  );
  const binaries = [electron, realEngine, cliBin, win ? `${ringFixture}.exe` : ringFixture];
  const exes = new Set(binaries.map(norm));
  const names = [...new Set([...binaries.map((b) => path.basename(b)), win ? "node.exe" : "node"])];
  const support = norm(path.join(desktopDir, "test-support"));
  // Electron and the engine also run the developer's own app from this checkout (`pnpm dev`),
  // which can start new processes during a run (a renderer after a reload, an engine restart).
  // Only those carrying a suite's marker are ours: the throwaway profile (`cutweave-e2e-*`, in
  // the user-data, run and log folders of the app and its engine) or Playwright's loader.
  const marked = new Set([norm(electron), norm(realEngine)]);
  const hasMarker = (p: Proc) => /cutweave-e2e-|playwright/i.test(p.cmd);
  const isOurs = (p: Proc) =>
    (p.exe !== "" && exes.has(norm(p.exe)) && (!marked.has(norm(p.exe)) || hasMarker(p))) ||
    (/^node/i.test(p.name) && (win ? p.cmd.toLowerCase() : p.cmd).includes(support));
  return () => processes(names).filter(isOurs);
}

function kill(pid: number): void {
  if (win) spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
  else {
    try {
      process.kill(pid, "SIGKILL");
    } catch {
      // already gone
    }
  }
}

/** How long processes may take to exit after the last app closed. */
const GRACE_MS = 10_000;

export default function globalSetup(): () => Promise<void> {
  const list = ourProcesses();
  const before = new Set(list().map((p) => p.pid));
  return async () => {
    const left = () => list().filter((p) => !before.has(p.pid));
    const deadline = Date.now() + GRACE_MS;
    let remaining = left();
    while (remaining.length > 0 && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 500));
      remaining = left();
    }
    if (remaining.length === 0) return;
    for (const p of remaining) kill(p.pid);
    throw new Error(
      `the e2e run left ${remaining.length} process(es) running (now killed):\n` +
        remaining.map((p) => `  ${p.pid} ${p.name} ${p.cmd || p.exe}`).join("\n"),
    );
  };
}
