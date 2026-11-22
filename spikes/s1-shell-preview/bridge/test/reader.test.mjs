// SPDX-License-Identifier: GPL-3.0-or-later
// Node tests for the s1-bridge addon reader (WP-0C.01 "Node test for addon reader").
// Run: pnpm test   (needs `pnpm build:native` first)
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { badRowStamps, sampledFnv1a } from "../../desktop/src/shared/integrity.ts";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");
const require = createRequire(import.meta.url);
/** @type {typeof import("../index.d.ts")} */
const bridge = require(join(here, "..", "s1_bridge.node"));

const RGBA8 = 1;
const NV12 = 2;
const W = 64;
const H = 16;
const uniqueName = () => `cw-s1-t-${randomBytes(4).toString("hex")}`;

test("TS checksum mirror matches Rust (known vector + random data)", () => {
  const known = new Uint8Array(4096);
  const dv = new DataView(known.buffer);
  for (let i = 0; i < 1024; i++) dv.setUint32(i * 4, i * 7, true);
  assert.equal(sampledFnv1a(known), 0x1b54ff1d);
  assert.equal(bridge.checksum(known, known.length), 0x1b54ff1d);
  const rnd = new Uint8Array(randomBytes(100_003));
  assert.equal(sampledFnv1a(rnd), bridge.checksum(rnd, rnd.length));
});

test("reader follows the writer across several wrap-arounds", () => {
  const name = uniqueName();
  const w = bridge.TestRingWriter.create(name, 4, W, H);
  const r = bridge.RingReader.open(name);
  assert.equal(r.slotCount, 4);
  assert.equal(r.readLatestInto(new Uint8Array(r.capacity), 0), null, "empty ring");
  const dst = new Uint8Array(r.capacity);
  let last = 0;
  for (let i = 0; i < 23; i++) {
    const seq = w.publish(RGBA8, i * 11_760_000, i);
    const h = r.readLatestInto(dst, last);
    assert.ok(h, `frame ${i}`);
    assert.equal(h.seq, seq);
    assert.ok(h.seq > last, "seq strictly increasing");
    assert.equal(h.slot, i % 4, "slot index wraps");
    assert.equal(h.pts, i * 11_760_000);
    assert.equal(h.width, W);
    assert.equal(h.format, RGBA8);
    assert.equal(h.payloadLen, W * H * 4);
    assert.ok(h.readUs >= 0);
    const bytes = dst.subarray(0, h.payloadLen);
    assert.equal(sampledFnv1a(bytes), h.checksum, "checksum");
    assert.equal(badRowStamps(bytes, h.stride, h.height, h.seq), 0, "row stamps");
    assert.equal(bytes[8], i & 0xff, "pixel data after the row stamp");
    assert.equal(r.readLatestInto(dst, h.seq), null, "nothing newer");
    last = h.seq;
  }
  const hdr = r.readHeader();
  assert.equal(hdr.writeIndex, 23);
  assert.equal(hdr.slotCount, 4);
  assert.deepEqual(hdr.slots.map((s) => s?.seq).sort((a, b) => a - b), [20, 21, 22, 23]);
  assert.equal(r.stats().frames, 23);
  r.close();
  w.close();
});

test("NV12 payload size and stride", () => {
  const name = uniqueName();
  const w = bridge.TestRingWriter.create(name, 6, W, H);
  const r = bridge.RingReader.open(name);
  w.publish(NV12, 0, 128);
  const dst = new Uint8Array(r.capacity);
  const h = r.readLatestInto(dst, 0);
  assert.ok(h);
  assert.equal(h.format, NV12);
  assert.equal(h.stride, W);
  assert.equal(h.payloadLen, W * H * 1.5);
  assert.equal(sampledFnv1a(dst.subarray(0, h.payloadLen)), h.checksum);
  r.close();
  w.close();
});

test("a slot being written is never returned; corruption is caught by the checksum", () => {
  const name = uniqueName();
  const w = bridge.TestRingWriter.create(name, 4, W, H);
  const r = bridge.RingReader.open(name);
  const dst = new Uint8Array(r.capacity);
  w.publish(RGBA8, 0, 1);
  w.beginWrite(0);
  assert.equal(r.readLatestInto(dst, 0), null);
  assert.equal(r.stats().torn, 1);
  assert.equal(r.readHeader().slots[0], null);
  w.endWrite(0);
  w.corrupt(0, 0);
  const h = r.readLatestInto(dst, 0);
  assert.ok(h);
  assert.notEqual(sampledFnv1a(dst.subarray(0, h.payloadLen)), h.checksum);
  assert.equal(badRowStamps(dst, h.stride, h.height, h.seq), 1);
  r.close();
  w.close();
});

test("latestView is zero-copy in plain Node and is invalidated when the slot is reused", () => {
  const name = uniqueName();
  const w = bridge.TestRingWriter.create(name, 4, W, H);
  const r = bridge.RingReader.open(name);
  w.publish(RGBA8, 0, 9);
  const v = r.latestView();
  assert.ok(v);
  assert.equal(v.zeroCopy, true, "Node allows external ArrayBuffers");
  assert.equal(v.buffer.byteLength, W * H * 4);
  assert.equal(new Uint8Array(v.buffer)[8], 9);
  assert.equal(r.isValid(v.header), true);
  for (let i = 0; i < 4; i++) w.publish(RGBA8, 0, 10 + i);
  assert.equal(r.isValid(v.header), false, "writer lapped the ring");
  // The view aliases shared memory, so it now shows the newer frame's bytes.
  assert.equal(new Uint8Array(v.buffer)[8], 13);
  r.close();
  w.close();
});

test("closing the last handle releases the named object", () => {
  const name = uniqueName();
  const w = bridge.TestRingWriter.create(name, 4, W, H);
  const r = bridge.RingReader.open(name);
  assert.equal(bridge.ringExists(name), true);
  w.close();
  assert.equal(bridge.ringExists(name), true, "reader still maps it");
  r.close();
  assert.equal(r.closed, true);
  assert.equal(bridge.ringExists(name), false);
  assert.throws(() => r.readHeader(), /closed/);
  assert.throws(() => bridge.RingReader.open(name), /not found/);
});

const exe = join(root, "target", "release", process.platform === "win32" ? "s1-engine.exe" : "s1-engine");

test("end to end with the real engine: handshake, frames, control channel, exit on stdin EOF", { skip: !existsSync(exe) && "engine not built", timeout: 30_000 }, async () => {
  const ring = uniqueName();
  const child = spawn(exe, ["--ring", ring, "--width", "640", "--height", "360"], {
    stdio: ["pipe", "pipe", "inherit"],
  });
  const exited = new Promise((resolve) => child.on("exit", (code) => resolve(code)));
  // A failed assertion must not leave the engine running: its pipes would keep the test
  // runner alive forever.
  let done = false;
  child.on("exit", () => {
    done = true;
  });
  try {
    const line = await new Promise((resolve, reject) => {
      createInterface({ input: child.stdout }).once("line", resolve);
      child.once("exit", () => reject(new Error("engine exited before handshake")));
    });
    const hs = JSON.parse(line);
    assert.equal(hs.s1, "ready");
    assert.match(hs.token, /^[0-9a-f]{64}$/, "256-bit token");
    assert.equal(hs.ring, ring);

    const r = bridge.RingReader.open(ring);
    const dst = new Uint8Array(r.capacity);
    let last = 0;
    let frames = 0;
    const until = Date.now() + 1000;
    while (Date.now() < until) {
      const h = r.readLatestInto(dst, last);
      if (h) {
        assert.ok(h.seq > last);
        assert.equal(sampledFnv1a(dst.subarray(0, h.payloadLen)), h.checksum);
        assert.equal(badRowStamps(dst, h.stride, h.height, h.seq), 0);
        last = h.seq;
        frames++;
      }
      await new Promise((res) => setTimeout(res, 4));
    }
    assert.ok(frames >= 40, `got ${frames} frames in 1 s`);

    // Control channel: wrong token is rejected, right token can pause / seek / switch format.
    const ws = (url) =>
      new Promise((resolve, reject) => {
        const s = new WebSocket(url);
        s.onopen = () => resolve(s);
        s.onerror = reject;
      });
    const next = (s) => new Promise((resolve) => s.addEventListener("message", (e) => resolve(JSON.parse(e.data)), { once: true }));
    const bad = await ws(`ws://127.0.0.1:${hs.port}/`);
    bad.send(JSON.stringify({ type: "auth", token: "00".repeat(32) }));
    assert.equal((await next(bad)).type, "error");

    const s = await ws(`ws://127.0.0.1:${hs.port}/`);
    s.send(JSON.stringify({ type: "auth", token: hs.token }));
    assert.equal((await next(s)).type, "hello");
    s.send(JSON.stringify({ type: "pause" }));
    assert.deepEqual(await next(s), { type: "ack", cmd: "pause" });
    await new Promise((res) => setTimeout(res, 100));
    assert.equal(r.readHeader().engineState, 2, "paused");
    const seekPts = 705_600_000 * 2; // 2 s
    s.send(JSON.stringify({ type: "seek", pts: seekPts }));
    await next(s);
    s.send(JSON.stringify({ type: "format", format: "nv12" }));
    await next(s);
    await new Promise((res) => setTimeout(res, 150));
    const h = r.latestHeader();
    assert.equal(h.pts, seekPts, "paused seek renders exactly the requested frame");
    assert.equal(h.format, NV12);

    // WP-0C.02: resize renegotiates the frame size (rounded for NV12) without remapping.
    const poll = async (what, pred, ms) => {
      const end = Date.now() + ms;
      for (;;) {
        const x = r.latestHeader();
        if (x && pred(x)) return x;
        if (Date.now() > end) assert.fail(`timeout waiting for ${what}`);
        await new Promise((res) => setTimeout(res, 2));
      }
    };
    s.send(JSON.stringify({ type: "resize", width: 803, height: 451 }));
    assert.deepEqual(await next(s), { type: "ack", cmd: "resize" });
    const hr = await poll("resized frame", (x) => x.width !== 640, 2000);
    assert.deepEqual([hr.width, hr.height, hr.stride], [800, 450, 800], "width % 4, even height");
    assert.equal(hr.pts, seekPts, "paused: the current frame is re-rendered at the new size");

    // WP-0C.02: a paused seek publishes a low-res preview first (flags bit 0), then the
    // full-resolution frame of the same pts once the (emulated, 300 ms) decode is done.
    s.send(JSON.stringify({ type: "scrub-config", lowres_div: 2, full_cost_ms: 300 }));
    assert.deepEqual(await next(s), { type: "ack", cmd: "scrub-config" });
    const seek2 = 705_600_000 * 3;
    const t0 = Date.now();
    s.send(JSON.stringify({ type: "seek", pts: seek2 }));
    await next(s);
    const lo = await poll("low-res preview", (x) => x.pts === seek2, 2000);
    assert.equal(lo.flags & 1, 1, "first frame for the seek is the low-res preview");
    assert.deepEqual([lo.width, lo.height], [400, 224]);
    const full = await poll("full-resolution frame", (x) => x.pts === seek2 && x.flags === 0, 3000);
    assert.deepEqual([full.width, full.height], [800, 450]);
    assert.ok(Date.now() - t0 >= 250, "full resolution waited for the emulated decode");
    s.close();
    bad.close();

    child.stdin.end(); // parent "dies"
    assert.equal(await exited, 0);
    r.close();
    assert.equal(bridge.ringExists(ring), false, "no leaked ring after engine exit + reader close");
  } finally {
    if (!done) child.kill();
  }
});
