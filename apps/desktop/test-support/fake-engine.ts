// SPDX-License-Identifier: GPL-3.0-or-later
// Fake `cutweave engine` for dev and tests: speaks the launch protocol (WP-1I.04) only.
//   node fake-engine.ts engine --port N --token-stdin
// First stdin line = UI token (kept in memory, never printed); stdout gets the readiness line;
// exits when stdin closes (orphan prevention); exit 12 when the port cannot be bound.
// Test knobs: FAKE_ENGINE_MODE=no-ready | exit-early | crash-after-ready.

import { createServer } from "node:http";
import { createInterface } from "node:readline";

const args = process.argv.slice(2);
const portIndex = args.indexOf("--port");
const port = Number(args[portIndex + 1]);
const mode = process.env.FAKE_ENGINE_MODE ?? "";

if (!Number.isInteger(port) || port < 0) process.exit(2);
if (mode === "exit-early") process.exit(1);

const input = createInterface({ input: process.stdin });
let token: string | null = null;

input.once("line", (line) => {
  token = line.trim();
  if (token.length === 0) process.exit(13);
  if (mode === "no-ready") return;

  const server = createServer((_req, res) => {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ ok: true }));
  });
  server.once("error", () => process.exit(12));
  server.listen(port, "127.0.0.1", () => {
    const address = server.address();
    const bound = typeof address === "object" && address !== null ? address.port : port;
    process.stdout.write(
      `${JSON.stringify({ event: "ready", port: bound, pid: process.pid, version: "fake" })}\n`,
    );
    if (mode === "crash-after-ready") setTimeout(() => process.exit(70), 50);
  });
});
input.once("close", () => {
  if (token === null) process.exit(13);
  process.exit(0);
});
