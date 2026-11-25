// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1H.03 Node harness: the built `cutweave_bridge.node` against a real Rust writer
// (crates/cutweave-bridge/examples/ring_fixture.rs, which publishes through the engine's
// `FramePublisher`). Runs in plain Node, outside Electron: the same addon call that is zero-copy
// here is one V8 copy in Electron (memory cage), so these tests assert correctness, pooling and
// the copy cost, not zero-copy. Missing artifacts are built on demand (`just bridge`).

import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type * as Native from "../../native/index.d.ts";

const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const repoRoot = path.resolve(desktopDir, "..", "..");
const profile = process.env.CUTWEAVE_BRIDGE_PROFILE === "release" ? "release" : "debug";
const targetDir = process.env.CARGO_TARGET_DIR ?? path.join(repoRoot, "target");
const addonPath = path.join(desktopDir, "native", "cutweave_bridge.node");
const fixturePath = path.join(
  targetDir,
  profile,
  "examples",
  process.platform === "win32" ? "ring_fixture.exe" : "ring_fixture",
);

let native: typeof Native;

beforeAll(() => {
  if (!existsSync(addonPath) || !existsSync(fixturePath)) {
    const args = [
      "scripts/build-bridge.ts",
      "--fixture",
      ...(profile === "release" ? ["--release"] : []),
    ];
    const built = spawnSync(process.execPath, args, { cwd: desktopDir, stdio: "inherit" });
    if (built.status !== 0) throw new Error("building the bridge addon failed (see output above)");
  }
  native = createRequire(import.meta.url)(addonPath) as typeof Native;
}, 600_000);

interface Announcement {
  event: string;
  name: string;
  epoch: string;
  pid: number;
  width: number;
  height: number;
  format: string;
  slotCapacity: number;
}

/** A running fixture writer: announcement, event stream, commands on stdin. */
class Fixture {
  readonly events: Announcement[] = [];
  private waiters: (() => void)[] = [];

  private constructor(
    readonly child: ChildProcess,
    readonly ready: Announcement,
  ) {}

  static async start(args: string[] = []): Promise<Fixture> {
    const child = spawn(fixturePath, args, { stdio: ["pipe", "pipe", "inherit"] });
    const lines = createInterface({ input: child.stdout as NodeJS.ReadableStream });
    const first = await new Promise<Announcement>((resolve, reject) => {
      child.once("error", reject);
      child.once("exit", (code) => reject(new Error(`fixture exited early (${code})`)));
      lines.once("line", (line) => resolve(JSON.parse(line) as Announcement));
    });
    const fixture = new Fixture(child, first);
    lines.on("line", (line) => {
      fixture.events.push(JSON.parse(line) as Announcement);
      for (const w of fixture.waiters.splice(0)) w();
    });
    return fixture;
  }

  send(command: string): void {
    this.child.stdin?.write(`${command}\n`);
  }

  async event(name: string): Promise<Announcement> {
    for (;;) {
      const hit = this.events.find((e) => e.event === name);
      if (hit) return hit;
      await new Promise<void>((resolve) => this.waiters.push(resolve));
    }
  }

  async stop(): Promise<void> {
    if (this.child.exitCode === null) {
      this.send("quit");
      await new Promise<void>((resolve) => {
        const t = setTimeout(() => {
          this.child.kill();
          resolve();
        }, 3000);
        this.child.once("exit", () => {
          clearTimeout(t);
          resolve();
        });
      });
    }
  }
}

const fixtures: Fixture[] = [];
async function start(args: string[] = []): Promise<Fixture> {
  const f = await Fixture.start(args);
  fixtures.push(f);
  return f;
}
afterAll(async () => {
  await Promise.all(fixtures.map((f) => f.stop()));
});

/** Check a fixture frame: seq stamp in the first 4 bytes, per-row value elsewhere. */
function verify(frame: Native.PreviewFrame): string | null {
  const bytes = new Uint8Array(frame.bytes);
  const stamp = new DataView(frame.bytes).getUint32(0, true);
  if (stamp !== frame.seq % 2 ** 32) return `seq stamp ${stamp} != ${frame.seq}`;
  const stride = frame.stride;
  const rows = Math.floor(bytes.length / stride);
  const step = Math.max(1, Math.floor(rows / 64));
  for (let row = 0; row < rows; row += step) {
    const want = (frame.seq * 3 + row) & 0xff;
    const start = row * stride;
    for (const off of [
      start + (row === 0 ? 8 : 0),
      start + Math.floor(stride / 2),
      start + stride - 1,
    ]) {
      if (off >= bytes.length) continue;
      if (bytes[off] !== want) return `row ${row} byte ${off} is ${bytes[off]}, want ${want}`;
    }
  }
  return null;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until<T>(
  what: string,
  ms: number,
  probe: () => T | null | undefined | false,
): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = probe();
    if (v) return v as T;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(5);
  }
}

describe("cutweave-bridge addon against the Rust writer", () => {
  // Correctness (no torn reads, one pooled copy each) is checked over a fixed number of distinct
  // frames, not over a time window: how many frames a loaded machine reads in 10 s says nothing
  // about tearing. The 60 fps throughput budget is its own test below.
  const WANTED_FRAMES = 150;
  for (const mode of ["correctness", "throughput"] as const) {
    const strict = mode === "throughput";
    const name = strict
      ? "reads 590+ distinct 1080p frames in 10 s at 60 fps (a budget for an idle machine: CUTWEAVE_STRICT_TIMING=1 enforces it, otherwise it is reported)"
      : `reads ${WANTED_FRAMES} distinct 1080p frames at 60 fps: no torn reads, one pooled copy each`;
    it(name, { timeout: 120_000 }, async () => {
      const fx = await start(["--width", "1920", "--height", "1080", "--fps", "60"]);
      const ring = native.openRing(fx.ready.name, fx.ready.epoch);
      const info = ring.header();
      expect(info).toMatchObject({ name: fx.ready.name, epoch: fx.ready.epoch, layoutVersion: 1 });
      expect(info.slotCount).toBe(6);

      const arrayBuffersBefore = process.memoryUsage().arrayBuffers;
      const seen = new Set<number>();
      const bad: string[] = [];
      const copyMs: number[] = [];
      let lastSeq = 0;
      let peakArrayBuffers = 0;
      // Throughput: a 10 s window. Correctness: until enough frames, with a deadline only a stuck
      // writer reaches.
      const end = Date.now() + (strict ? 10_000 : 90_000);
      while (Date.now() < end && (strict || seen.size < WANTED_FRAMES)) {
        await ring.waitForFrame(50);
        const t0 = performance.now();
        const frame = ring.latestFrame();
        const dt = performance.now() - t0;
        if (frame === null) continue;
        copyMs.push(dt);
        // Zero torn reads: every delivered frame is internally consistent and newer than the last.
        expect(frame.seq).toBeGreaterThan(lastSeq);
        lastSeq = frame.seq;
        seen.add(frame.seq);
        expect([frame.width, frame.height, frame.format]).toEqual([1920, 1080, "rgba8"]);
        expect(frame.bytes.byteLength).toBe(1920 * 1080 * 4);
        expect(frame.stride).toBe(1920 * 4);
        const problem = verify(frame);
        if (problem) bad.push(`seq ${frame.seq}: ${problem}`);
        peakArrayBuffers = Math.max(peakArrayBuffers, process.memoryUsage().arrayBuffers);
      }
      const status = ring.status();
      ring.close();

      expect(bad).toEqual([]);
      if (strict && process.env.CUTWEAVE_STRICT_TIMING === "1") {
        expect(seen.size).toBeGreaterThanOrEqual(590);
      } else {
        expect(seen.size).toBeGreaterThanOrEqual(strict ? 1 : WANTED_FRAMES);
      }
      expect(status.alive).toBe(true);
      // Copy once into pooled buffers: no per-frame allocation, so ArrayBuffer memory stays flat
      // (two pooled 8.3 MB buffers, nothing proportional to the 600 frames).
      expect(peakArrayBuffers - arrayBuffersBefore).toBeLessThan(20 * 1024 * 1024);
      copyMs.sort((a, b) => a - b);
      const p = (q: number) => copyMs[Math.floor(q * (copyMs.length - 1))]?.toFixed(2);
      console.log(
        `[bridge] ${seen.size} distinct frames in 10 s; latestFrame() 1080p RGBA8 (${profile}): p50 ${p(0.5)} ms, p95 ${p(0.95)} ms, max ${p(1)} ms; torn copies retried by the seqlock: ${status.tornRetries}`,
      );
    });
  }

  it("pools two buffers per frame size (alternating) so the previous frame stays valid", async () => {
    const fx = await start(["--width", "320", "--height", "180", "--fps", "120"]);
    const ring = native.openRing(fx.ready.name, BigInt(fx.ready.epoch));
    const take = async () => {
      for (;;) {
        await ring.waitForFrame(200);
        const f = ring.latestFrame();
        if (f) return f;
      }
    };
    const a = await take();
    const aSeq = a.seq;
    const aStamp = new DataView(a.bytes).getUint32(0, true);
    const b = await take();
    expect(b.bytes).not.toBe(a.bytes);
    // `a` is untouched by reading `b` ...
    expect(new DataView(a.bytes).getUint32(0, true)).toBe(aStamp);
    expect(aStamp).toBe(aSeq % 2 ** 32);
    // ... and the buffer comes back after two calls.
    const c = await take();
    expect(c.bytes).toBe(a.bytes);
    ring.close();
  });

  it("rejects a ring whose epoch is not the announced one", async () => {
    const fx = await start(["--width", "64", "--height", "64"]);
    const wrong = (BigInt(fx.ready.epoch) ^ 1n).toString();
    expect(() => native.openRing(fx.ready.name, wrong)).toThrow(/^ERR_RING_EPOCH/);
    expect(() => native.openRing(fx.ready.name, "not a number")).toThrow(/^ERR_RING_EPOCH/);
    native.openRing(fx.ready.name, fx.ready.epoch).close();
  });

  it("throws typed errors for a missing or malformed ring and after close()", async () => {
    expect(() => native.openRing("cwp-00000001-0000000000000001", 1n)).toThrow(
      /^ERR_RING_NOT_FOUND/,
    );
    expect(() => native.openRing("nope", 1n)).toThrow(/^ERR_RING_INVALID/);
    const fx = await start(["--width", "64", "--height", "64"]);
    const ring = native.openRing(fx.ready.name, fx.ready.epoch);
    ring.close();
    ring.close(); // idempotent
    expect(() => ring.latestFrame()).toThrow(/^ERR_RING_CLOSED/);
    expect(() => ring.header()).toThrow(/^ERR_RING_CLOSED/);
    expect(await ring.waitForFrame(10)).toBe("ended");
  });

  it("follows Retired -> successor ring when the engine renegotiates, seq keeps increasing", async () => {
    const fx = await start(["--width", "320", "--height", "180", "--fps", "60"]);
    const ring = native.openRing(fx.ready.name, fx.ready.epoch);
    const first = await until("a first frame", 3000, () => ring.latestFrame());
    expect([first.width, first.height]).toEqual([320, 180]);
    fx.send("resize 1280 720");
    const moved = await fx.event("moved");
    expect(moved.name).not.toBe(fx.ready.name);
    let seen: Native.PreviewFrame | null = null;
    let lastSeq = first.seq;
    await until("a frame from the successor ring", 5000, () => {
      const f = ring.latestFrame();
      if (f) {
        expect(f.seq).toBeGreaterThan(lastSeq);
        lastSeq = f.seq;
        if (f.width === 1280) seen = f;
      }
      return seen !== null;
    });
    expect(seen).not.toBeNull();
    expect(verify(seen as unknown as Native.PreviewFrame)).toBeNull();
    const status = ring.status();
    expect(status).toMatchObject({ state: "live", alive: true, switches: 1, name: moved.name });
    expect(ring.header().epoch).toBe(moved.epoch);
    ring.close();
  }, 30_000);

  it("picks frames by the audio clock: pts <= clock - latency, once each", {
    timeout: 120_000,
  }, async () => {
    const fx = await start(["--width", "160", "--height", "90", "--fps", "60"]);
    const ring = native.openRing(fx.ready.name, fx.ready.epoch);
    // The fixture publishes a stopped clock before it starts playing; under load the first
    // sample can still be that one, so wait for the playing clock rather than the first sample.
    const first = await until("a playing clock sample", 3000, () => {
      const sample = ring.audioClock();
      return sample?.state === "playing" ? sample : null;
    });
    expect(first.state).toBe("playing");
    expect(first.rateMilli).toBe(1000);
    let lastSeq = 0;
    let n = 0;
    // Until 61 frames were picked (not for a fixed time: a loaded machine picks fewer per second,
    // which says nothing about the clock scheduling), with a deadline only a stuck writer reaches.
    const end = Date.now() + 60_000;
    while (n <= 60 && Date.now() < end) {
      const clock = ring.audioClock();
      const frame = ring.frameAtClock(20);
      if (clock && frame) {
        const margin = (20 / 1000) * 705_600_000;
        // The frame was chosen at some instant between the two clock reads; the clock only
        // moves forward, so the later reading bounds it.
        const after = ring.audioClock();
        expect(after).not.toBeNull();
        expect(after && clock.ptsFlicks <= after.ptsFlicks).toBe(true);
        expect(frame.pts).toBeLessThanOrEqual((after?.ptsFlicks ?? 0) - margin);
        expect(frame.seq).toBeGreaterThan(lastSeq);
        lastSeq = frame.seq;
        n += 1;
      }
      await sleep(2);
    }
    expect(n).toBeGreaterThan(60);
    expect(ring.audioClock()?.ptsFlicks).toBeGreaterThan(first.ptsFlicks);
    ring.close();
  });

  it("waitForFrame resolves on a new frame, on timeout, and on retirement", async () => {
    const fx = await start(["--width", "64", "--height", "64", "--fps", "30"]);
    const ring = native.openRing(fx.ready.name, fx.ready.epoch);
    expect(await ring.waitForFrame(1000)).toBe("frame");
    await until("frame", 2000, () => ring.latestFrame());
    fx.send("idle");
    // Drain until the writer is really idle (on a loaded machine the command and frames already in
    // flight take longer than a fixed sleep to settle); the timeout below is the assertion.
    while ((await ring.waitForFrame(150)) === "frame") ring.latestFrame();
    const t0 = performance.now();
    expect(await ring.waitForFrame(150)).toBe("timeout");
    expect(performance.now() - t0).toBeGreaterThanOrEqual(140);
    fx.send("resize 1024 1024");
    expect(await ring.waitForFrame(2000)).toBe("changed");
    ring.close();
  });

  it("treats a hung writer as dead after 500 ms without heartbeat and releases the ring", async () => {
    const fx = await start(["--width", "64", "--height", "64", "--fps", "30"]);
    const ring = native.openRing(fx.ready.name, fx.ready.epoch);
    await until("a frame", 2000, () => ring.latestFrame());
    expect(ring.status().alive).toBe(true);
    fx.send("idle"); // an idle engine keeps beating
    await sleep(900);
    expect(ring.status()).toMatchObject({ state: "live", alive: true });
    fx.send("hang");
    await sleep(200);
    expect(ring.status().alive).toBe(true);
    const hungAt = performance.now();
    const dead = await until("the writer to be declared dead", 3000, () => {
      const s = ring.status();
      return s.state === "ended" ? s : false;
    });
    expect(dead).toMatchObject({ alive: false, reason: "heartbeat-stale" });
    expect(performance.now() - hungAt).toBeLessThan(1500);
    expect(ring.latestFrame()).toBeNull();
    expect(() => ring.header()).toThrow(/^ERR_RING_CLOSED/);
    ring.close();
  });

  it("detects a killed engine, and a cleanly closed ring", async () => {
    const killed = await start(["--width", "64", "--height", "64", "--fps", "30"]);
    const r1 = native.openRing(killed.ready.name, killed.ready.epoch);
    await until("a frame", 2000, () => r1.latestFrame());
    killed.child.kill("SIGKILL");
    const s1 = await until("the killed engine to be detected", 3000, () => {
      const s = r1.status();
      return s.state === "ended" ? s : false;
    });
    expect(s1.reason).toBe("heartbeat-stale");

    const closing = await start(["--width", "64", "--height", "64", "--fps", "30"]);
    const r2 = native.openRing(closing.ready.name, closing.ready.epoch);
    await until("a frame", 2000, () => r2.latestFrame());
    closing.send("quit");
    const s2 = await until("the close to be noticed", 3000, () => {
      const s = r2.status();
      return s.state === "ended" ? s : false;
    });
    expect(s2.reason).toBe("writer-closed");
    r1.close();
    r2.close();
  });

  it("delivers NV12 planes and padded rows as published", async () => {
    const fx = await start([
      "--width",
      "640",
      "--height",
      "360",
      "--format",
      "nv12",
      "--row-align",
      "256",
      "--fps",
      "60",
    ]);
    const ring = native.openRing(fx.ready.name, fx.ready.epoch);
    const f = await until("an NV12 frame", 3000, () => ring.latestFrame());
    expect(f.format).toBe("nv12");
    expect(f.stride).toBe(768); // 640 rounded up to 256
    expect(f.plane1Stride).toBe(768);
    expect(f.plane1Offset).toBe(768 * 360);
    expect(f.bytes.byteLength).toBe(768 * 360 + 768 * 180);
    expect(verify(f)).toBeNull();
    ring.close();
  });

  it("counts what it did in perfCounters()", async () => {
    const before = native.perfCounters();
    const fx = await start(["--width", "100", "--height", "50", "--fps", "60"]);
    const ring = native.openRing(fx.ready.name, fx.ready.epoch);
    const frame = await until("a frame", 3000, () => ring.latestFrame());
    const after = native.perfCounters();
    expect(after.ringsOpened - before.ringsOpened).toBe(1);
    expect(after.framesRead - before.framesRead).toBe(1);
    expect(after.bytesCopied - before.bytesCopied).toBe(frame.bytes.byteLength);
    ring.close();
  });
});
