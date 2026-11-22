// SPDX-License-Identifier: GPL-3.0-or-later
// Fake `cutweave engine` for dev and tests: speaks the launch protocol (WP-1I.04) only.
//   node fake-engine.ts engine --port N --token-stdin
// First stdin line = UI token (kept in memory, never printed); stdout gets the readiness line;
// exits when stdin closes (orphan prevention); exit 12 when the port cannot be bound.
// HTTP: `/rpc` answers 401 without `Authorization: Bearer <token>`; everything else 200.
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

  const server = createServer((req, res) => {
    // The editor origin may read responses (CORS), as the real engine allows for `app://cutweave`.
    const headers = {
      "content-type": "application/json",
      "access-control-allow-origin": "app://cutweave",
    };
    // Like the real engine, /rpc needs the UI token (arch §6); there is no token in a URL.
    if (req.url?.startsWith("/rpc") && req.headers.authorization !== `Bearer ${token}`) {
      res.writeHead(401, headers);
      res.end(JSON.stringify({ error: "unauthorized" }));
      return;
    }
    res.writeHead(200, headers);
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
