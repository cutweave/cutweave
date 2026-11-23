// SPDX-License-Identifier: GPL-3.0-or-later
// `just dev`: Vite dev server for `packages/editor` (HMR) + watch-built main/preload + Electron,
// with the dev engine (`target/debug/cutweave`, else `target/debug/cutweave-engine-serve`) or,
// when neither is built, the fake engine. Main/preload changes restart Electron; renderer
// changes hot-reload.

import { type ChildProcess, spawn } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import electronPath from "electron";
import { build, createServer } from "vite";

const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(desktopDir, "..", "..");
const editorDir = path.join(repoRoot, "packages", "editor");

const server = await createServer({ root: editorDir, server: { strictPort: false } });
await server.listen();
const devUrl = server.resolvedUrls?.local[0];
if (devUrl === undefined) throw new Error("Vite dev server did not report a URL");
server.printUrls();

const env: NodeJS.ProcessEnv = { ...process.env, CUTWEAVE_DEV_URL: devUrl.replace(/\/$/, "") };
// Main resolves the same candidates (src/main/engine/launch.ts).
const devEngines = ["cutweave", "cutweave-engine-serve"].map((name) =>
  path.join(repoRoot, "target", "debug", process.platform === "win32" ? `${name}.exe` : name),
);
if (!env.CUTWEAVE_ENGINE_BIN && !devEngines.some((e) => existsSync(e))) {
  env.CUTWEAVE_ENGINE_FAKE = path.join(desktopDir, "test-support", "fake-engine.ts");
  console.log("dev: no dev engine in target/debug; using the fake engine (test-support)");
}

let electron: ChildProcess | null = null;
let shuttingDown = false;

function launch(): void {
  electron = spawn(String(electronPath), [desktopDir], { stdio: "inherit", env });
  const child = electron;
  child.once("exit", () => {
    if (electron === child && !shuttingDown) void shutdown(0);
  });
}

async function shutdown(code: number): Promise<void> {
  if (shuttingDown) return;
  shuttingDown = true;
  electron?.kill();
  await server.close();
  process.exit(code);
}

const watcher = await build({
  root: desktopDir,
  configFile: path.join(desktopDir, "vite.node.config.ts"),
  build: { watch: {} },
});
if (!("on" in watcher)) throw new Error("expected a watcher from `vite build --watch`");

let first = true;
watcher.on("event", (event) => {
  if (event.code === "ERROR") console.error(event.error);
  if (event.code !== "END") return;
  if (first) {
    first = false;
    launch();
  } else if (electron !== null) {
    const old = electron;
    electron = null;
    old.once("exit", launch);
    old.kill();
  }
});

process.on("SIGINT", () => void shutdown(0));
process.on("SIGTERM", () => void shutdown(0));
