// SPDX-License-Identifier: GPL-3.0-or-later
// Fake `cutweave engine` for dev and tests: speaks the launch protocol (WP-1I.04) and a sliver
// of the JSON-RPC wire (WP-1I.02), enough for the supervisor's control connection.
//   node fake-engine.ts engine --port N --token-stdin [--project P] [--allow-origin O]...
// First stdin line = UI token (kept in memory, never printed); stdout gets the readiness line;
// exits when stdin closes (orphan prevention); exit 12 when the port cannot be bound.
// HTTP: `/rpc` answers 401 without `Authorization: Bearer <token>`; everything else 200.
// WebSocket `/rpc` (token as subprotocol `cutweave.token.<t>` or bearer): `rpc.hello` (with this
// build's schema hash), `rpc.subscribe`, `project.open`, `project.recover`, and empty answers to
// the editor's start-up reads (`project.snapshot`, `view.get`, `job.list`, `project.connections`)
// and to `project.set_allowed_roots`; anything else is `not_found`.
// Test knobs: FAKE_ENGINE_MODE=no-ready | exit-early | crash-after-ready;
// FAKE_ENGINE_EXIT=<code> prints the protocol's error line and exits with it after the
// handshake; FAKE_ENGINE_RECOVERY=1 makes a `--project` start report unsaved work to recover;
// FAKE_ENGINE_SCHEMA_HASH=<h> reports another schema hash (the editor must refuse to edit).
// Export (WP-1M.08): `media.capabilities` (FAKE_ENGINE_ENCODERS=h264_nvenc,h264_amf, default one),
// `export.start` (a path with "fail" fails at 30%, one with "restart" drops to 0 once as a failed
// hardware encoder restarts in software; an existing file needs `overwrite`), `job.cancel`; the
// job ramps up over about 2 s with `job.progress` events and writes a small file on success.
// Preview (WP-1M.04, fake-preview.ts): the `view.*` transport, `playback.state`, a frame ring
// (FAKE_ENGINE_RING), still frames on `GET /media/frame`, and FAKE_ENGINE_PROJECT=three-clips.

import { createHash } from "node:crypto";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import type { Duplex } from "node:stream";
import { SCHEMA_HASH } from "../../../packages/protocol/src/generated/schema-hash.ts";
import { FakePreview, framePng } from "./fake-preview.ts";

const args = process.argv.slice(2);
const argValue = (name: string): string | null => {
  const i = args.indexOf(name);
  return i >= 0 ? (args[i + 1] ?? null) : null;
};
const port = Number(argValue("--port"));
const mode = process.env.FAKE_ENGINE_MODE ?? "";
const exitWith = Number(process.env.FAKE_ENGINE_EXIT ?? "");

if (!Number.isInteger(port) || port < 0) process.exit(2);
if (mode === "exit-early") process.exit(1);

const ERROR_CODES: Record<number, string> = {
  1: "start_failed",
  2: "usage",
  10: "project_locked",
  11: "gpu_init_failed",
  12: "port_in_use",
  13: "bad_handshake",
};

// The three-clip project is an open, saved project (an untitled document with clips would hold
// the window open behind the unsaved-changes guard).
let project: string | null =
  argValue("--project") ??
  (process.env.FAKE_ENGINE_PROJECT === "three-clips"
    ? path.join(tmpdir(), "cutweave-fake", "three-clips")
    : null);
let recoveryPending = project !== null && process.env.FAKE_ENGINE_RECOVERY === "1";

const input = createInterface({ input: process.stdin });
let token: string | null = null;

input.once("line", (line) => {
  token = line.trim();
  if (token.length === 0) process.exit(13);
  if (Number.isInteger(exitWith) && exitWith > 0) {
    const code = ERROR_CODES[exitWith] ?? "start_failed";
    const holder = exitWith === 10 ? { holder: { pid: 4242, host: "HOST", started: 1 } } : {};
    const message = `fake engine failure (${code})`;
    const errorLine = { event: "error", code, exit_code: exitWith, message, ...holder };
    process.stdout.write(`${JSON.stringify(errorLine)}\n`, () => process.exit(exitWith));
    return;
  }
  if (mode === "no-ready") return;
  preview = new FakePreview(broadcast);

  const server = createServer((req, res) => {
    // The editor origin may read responses (CORS), as the real engine allows for `app://cutweave`.
    const headers = {
      "content-type": "application/json",
      "access-control-allow-origin": "app://cutweave",
    };
    // A CORS preflight (the editor's frame fetch sends an Authorization header) carries no token.
    if (req.method === "OPTIONS") {
      res.writeHead(204, {
        ...headers,
        "access-control-allow-methods": "GET, POST, OPTIONS",
        "access-control-allow-headers": "authorization, content-type",
      });
      res.end();
      return;
    }
    const authorized = req.headers.authorization === `Bearer ${token}`;
    // Like the real engine, /rpc and /media need the UI token (arch §6); never in a URL.
    if ((req.url?.startsWith("/rpc") || req.url?.startsWith("/media")) && !authorized) {
      res.writeHead(401, headers);
      res.end(JSON.stringify({ error: "unauthorized" }));
      return;
    }
    if (req.url?.startsWith("/media/frame")) {
      const q = new URL(req.url, "http://127.0.0.1").searchParams;
      const png = framePng(
        Number(q.get("t") ?? 0),
        Number(q.get("w") ?? 2),
        Number(q.get("h") ?? 2),
      );
      res.writeHead(200, { ...headers, "content-type": "image/png", "cache-control": "no-store" });
      res.end(png);
      return;
    }
    res.writeHead(200, headers);
    res.end(JSON.stringify({ ok: true }));
  });
  server.on("upgrade", (req, socket) => upgrade(req, socket));
  server.once("error", () => process.exit(12));
  void preview.ready().then(() =>
    server.listen(port, "127.0.0.1", () => {
      const address = server.address();
      const bound = typeof address === "object" && address !== null ? address.port : port;
      process.stdout.write(
        `${JSON.stringify({ event: "ready", port: bound, pid: process.pid, version: "fake" })}\n`,
      );
      if (mode === "crash-after-ready") setTimeout(() => process.exit(70), 50);
    }),
  );
});
let preview: FakePreview | null = null;
process.on("exit", () => preview?.stop());
input.once("close", () => {
  if (token === null) process.exit(13);
  process.exit(0);
});

// ----- a minimal RFC 6455 server (text frames, no fragmentation) ------------------------------

interface Client {
  socket: Duplex;
  events: Set<string>;
}

const clients = new Set<Client>();
const WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

function upgrade(req: IncomingMessage, socket: Duplex): void {
  const offered = String(req.headers["sec-websocket-protocol"] ?? "")
    .split(",")
    .map((p) => p.trim())
    .find((p) => p === `cutweave.token.${token}`);
  const bearer = req.headers.authorization === `Bearer ${token}`;
  if (!req.url?.startsWith("/rpc") || (offered === undefined && !bearer)) {
    socket.end("HTTP/1.1 401 Unauthorized\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
    return;
  }
  const accept = createHash("sha1")
    .update(`${req.headers["sec-websocket-key"] ?? ""}${WS_GUID}`)
    .digest("base64");
  socket.write(
    [
      "HTTP/1.1 101 Switching Protocols",
      "Upgrade: websocket",
      "Connection: Upgrade",
      `Sec-WebSocket-Accept: ${accept}`,
      ...(offered === undefined ? [] : [`Sec-WebSocket-Protocol: ${offered}`]),
      "",
      "",
    ].join("\r\n"),
  );
  const client: Client = { socket, events: new Set() };
  clients.add(client);
  let buffer = Buffer.alloc(0);
  socket.on("data", (chunk: Buffer) => {
    buffer = Buffer.concat([buffer, chunk]);
    for (let frame = readFrame(buffer); frame !== null; frame = readFrame(buffer)) {
      buffer = buffer.subarray(frame.size);
      if (frame.opcode === 0x8) {
        socket.end();
        return;
      }
      if (frame.opcode === 0x1) handle(client, frame.payload.toString("utf8"));
    }
  });
  const drop = () => clients.delete(client);
  socket.on("close", drop);
  socket.on("error", drop);
}

function readFrame(buf: Buffer): { opcode: number; payload: Buffer; size: number } | null {
  if (buf.length < 2) return null;
  const opcode = (buf[0] ?? 0) & 0x0f;
  const masked = ((buf[1] ?? 0) & 0x80) !== 0;
  let length = (buf[1] ?? 0) & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buf.length < 4) return null;
    length = buf.readUInt16BE(2);
    offset = 4;
  } else if (length === 127) {
    if (buf.length < 10) return null;
    length = Number(buf.readBigUInt64BE(2));
    offset = 10;
  }
  const maskBytes = masked ? 4 : 0;
  if (buf.length < offset + maskBytes + length) return null;
  const mask = buf.subarray(offset, offset + maskBytes);
  const payload = Buffer.from(buf.subarray(offset + maskBytes, offset + maskBytes + length));
  if (masked) {
    for (let i = 0; i < payload.length; i++) {
      payload[i] = (payload[i] ?? 0) ^ (mask[i % 4] ?? 0);
    }
  }
  return { opcode, payload, size: offset + maskBytes + length };
}

function send(client: Client, message: unknown): void {
  const payload = Buffer.from(JSON.stringify(message), "utf8");
  let header: Buffer;
  if (payload.length < 126) {
    header = Buffer.from([0x81, payload.length]);
  } else if (payload.length < 65_536) {
    header = Buffer.alloc(4);
    header[0] = 0x81;
    header[1] = 126;
    header.writeUInt16BE(payload.length, 2);
  } else {
    header = Buffer.alloc(10);
    header[0] = 0x81;
    header[1] = 127;
    header.writeBigUInt64BE(BigInt(payload.length), 2);
  }
  client.socket.write(Buffer.concat([header, payload]));
}

// ----- JSON-RPC ---------------------------------------------------------------------------------

function projectStatus() {
  return {
    state: project === null ? "no_project" : "open",
    path: project,
    name: "fake",
    rev: 0,
    saved_rev: null,
    dirty: false,
    read_only: null,
    recovery: recoveryPending
      ? { records: 3, base_rev: 0, last_rev: 3, last_ts: 0, damaged: false }
      : null,
  };
}

function broadcast(event: string, params: unknown): void {
  for (const client of clients) {
    if (client.events.has(event) || client.events.has("*")) {
      send(client, { jsonrpc: "2.0", method: event, params });
    }
  }
}

// ----- export (WP-1M.08) -----------------------------------------------------------------------

interface FakeJob {
  id: string;
  path: string;
  state: "running" | "succeeded" | "failed" | "cancelled";
  fraction: number;
  message: string | null;
  error: string | null;
  cancelRequested: boolean;
  timer: ReturnType<typeof setInterval> | null;
  restarted: boolean;
}
const jobs = new Map<string, FakeJob>();
let jobCounter = 0;

const jobInfo = (job: FakeJob) => ({
  id: job.id,
  kind: "export",
  label: "export",
  media_id: null,
  state: job.state,
  progress: { fraction: job.fraction, message: job.message },
  cancel_requested: job.cancelRequested,
  error: job.error,
});

function finishJob(job: FakeJob, state: FakeJob["state"], error: string | null = null): void {
  if (job.timer !== null) clearInterval(job.timer);
  job.timer = null;
  job.state = state;
  job.error = error;
  if (state === "succeeded") {
    job.fraction = 1;
    job.message = "exported 60 frames (2.00 s) with libx264";
    mkdirSync(path.dirname(job.path), { recursive: true });
    writeFileSync(job.path, "fake mp4");
  } else {
    rmSync(job.path, { force: true });
  }
  broadcast("job.progress", jobInfo(job));
}

function startFakeJob(file: string): FakeJob {
  jobCounter += 1;
  const job: FakeJob = {
    id: `job_fake${jobCounter}`,
    path: file,
    state: "running",
    fraction: 0,
    message: null,
    error: null,
    cancelRequested: false,
    timer: null,
    restarted: false,
  };
  jobs.set(job.id, job);
  job.timer = setInterval(() => {
    if (job.cancelRequested) {
      finishJob(job, "cancelled", "cancelled");
      return;
    }
    job.fraction = Math.min(1, job.fraction + 0.05);
    if (file.includes("restart") && !job.restarted && job.fraction >= 0.4) {
      job.restarted = true;
      job.fraction = 0;
    }
    const frame = Math.round(job.fraction * 60);
    job.message = `frame ${frame} of 60, 24.5 fps, about 0:01 left${job.restarted ? " (again in software: h264_nvenc failed)" : ""}`;
    if (file.includes("fail") && job.fraction >= 0.3) {
      finishJob(job, "failed", "the encoder stopped: the disk is full");
      return;
    }
    if (job.fraction >= 1) {
      finishJob(job, "succeeded");
      return;
    }
    broadcast("job.progress", jobInfo(job));
  }, 100);
  return job;
}

function handle(client: Client, text: string): void {
  let msg: { id?: unknown; method?: unknown; params?: Record<string, unknown> };
  try {
    msg = JSON.parse(text);
  } catch {
    send(client, { jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error" } });
    return;
  }
  const { id, method, params } = msg;
  const ok = (result: unknown) => {
    if (id !== undefined) send(client, { jsonrpc: "2.0", id, result: { ...(result as object) } });
  };
  switch (method) {
    case "rpc.hello":
      ok({
        engine_version: "fake",
        schema_hash: process.env.FAKE_ENGINE_SCHEMA_HASH || SCHEMA_HASH,
        rev: 0,
        committed_rev: 0,
        actor: "user",
        project: projectStatus(),
        // The preview frame ring, when one runs (RING.md `RingAnnouncement`).
        ...(preview?.shm() === undefined ? {} : { shm: preview.shm() }),
      });
      return;
    case "rpc.subscribe": {
      const events = Array.isArray(params?.events) ? params.events : [];
      for (const e of events) if (typeof e === "string") client.events.add(e);
      ok({ events: [...client.events] });
      return;
    }
    case "project.open":
      project = typeof params?.path === "string" ? params.path : null;
      recoveryPending = false;
      ok({ project: projectStatus() });
      broadcast("doc.reset", {});
      broadcast("project.opened", projectStatus());
      return;
    case "project.snapshot":
      ok({
        rev: 0,
        committed_rev: 0,
        project: preview?.project ?? {
          meta: { name: "fake" },
          settings: {},
          media: {},
          sequences: {},
          custom: {},
        },
      });
      return;
    case "view.get":
      ok({
        playhead: 0,
        zoom: 100,
        scroll: { x: 0, y: 0 },
        selection: [],
        panel_layout: null,
        preview_quality: "full",
        master_gain: null,
      });
      return;
    case "view.set":
      ok({});
      return;
    case "job.list":
      ok({ jobs: [...jobs.values()].map(jobInfo), active: 0 });
      return;
    // Settings > Agents and MCP (WP-1J.05): no agent ever connects to the fake.
    case "project.connections":
      ok({ connections: [] });
      return;
    case "project.set_allowed_roots":
      ok({ roots: Array.isArray(params?.roots) ? params.roots : [] });
      return;
    case "media.capabilities":
      ok({
        available: true,
        hw_decode: [],
        hw_encode: (process.env.FAKE_ENGINE_ENCODERS ?? "h264_nvenc")
          .split(",")
          .filter((n) => n !== "")
          .map((implementation) => ({ device: "fake", codec: "h264", implementation })),
        codecs: [],
      });
      return;
    case "export.start": {
      const file = typeof params?.path === "string" ? params.path : "";
      const refuse = (code: string, message: string, at?: string) => {
        if (id === undefined) return;
        send(client, {
          jsonrpc: "2.0",
          id,
          error: { code: -32000, message, data: { code, message, ...(at ? { path: at } : {}) } },
        });
      };
      if (!file.toLowerCase().endsWith(".mp4")) {
        refuse("invalid", "the output must be an .mp4 file", "/path");
        return;
      }
      if (existsSync(file) && params?.overwrite !== true) {
        refuse("invalid", `${file} already exists; pass overwrite to replace it`, "/overwrite");
        return;
      }
      const job = startFakeJob(file);
      ok({ job_id: job.id, path: file, frames: 60 });
      broadcast("job.progress", jobInfo(job));
      return;
    }
    case "job.cancel": {
      const job = jobs.get(String(params?.id));
      if (job === undefined) {
        ok({});
        return;
      }
      job.cancelRequested = true;
      ok(jobInfo(job));
      return;
    }
    case "project.recover": {
      const applied = recoveryPending ? 3 : 0;
      recoveryPending = false;
      ok({ project: projectStatus(), applied, damaged: false });
      broadcast("doc.reset", {});
      return;
    }
    default: {
      const answer =
        typeof method === "string" && method.startsWith("view.")
          ? preview?.handle(method, params)
          : undefined;
      if (answer !== undefined) {
        ok(answer);
        return;
      }
      if (id === undefined) return;
      send(client, {
        jsonrpc: "2.0",
        id,
        error: {
          code: -32601,
          message: `no such method: ${String(method)}`,
          data: { code: "not_found", message: "no such method" },
        },
      });
    }
  }
}
