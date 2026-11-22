// SPDX-License-Identifier: GPL-3.0-or-later
// Node tests for RingReader.waitForFrame (WP-0C.06, transport (b) wake-up primitive).
// Run: pnpm test   (needs `pnpm build:native` first)
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
/** @type {typeof import("../index.d.ts")} */
const bridge = require(join(here, "..", "s1_bridge.node"));

const RGBA8 = 1;
const uniqueName = () => `cw-s1-w-${randomBytes(4).toString("hex")}`;

test("waitForFrame resolves immediately when a newer frame exists", async () => {
  const name = uniqueName();
  const w = bridge.TestRingWriter.create(name, 4, 64, 16);
  const r = bridge.RingReader.open(name);
  w.publish(RGBA8, 0, 1);
  w.publish(RGBA8, 11_760_000, 2);
  const t0 = performance.now();
  assert.equal(await r.waitForFrame(1, 1000), 2);
  assert.ok(performance.now() - t0 < 50);
  r.close();
  w.close();
});

test("waitForFrame times out with the unchanged write index", async () => {
  const name = uniqueName();
  const w = bridge.TestRingWriter.create(name, 4, 64, 16);
  const r = bridge.RingReader.open(name);
  w.publish(RGBA8, 0, 1);
  const t0 = performance.now();
  assert.equal(await r.waitForFrame(1, 30), 1);
  const ms = performance.now() - t0;
  assert.ok(ms >= 25 && ms < 500, `timed out after ${ms} ms`);
  r.close();
  w.close();
});

test("waitForFrame wakes within a few ms of a publish (no 15.6 ms timer quantum)", async () => {
  const name = uniqueName();
  const w = bridge.TestRingWriter.create(name, 4, 64, 16);
  const r = bridge.RingReader.open(name);
  const lags = [];
  for (let i = 1; i <= 20; i++) {
    const pending = r.waitForFrame(i - 1, 1000);
    // Publish from a timer so the wait is already polling.
    const published = await new Promise((resolve) =>
      setTimeout(() => {
        w.publish(RGBA8, i * 11_760_000, i);
        resolve(performance.now());
      }, 5),
    );
    assert.equal(await pending, i);
    lags.push(performance.now() - published);
  }
  lags.sort((a, b) => a - b);
  const p50 = lags[Math.floor(lags.length / 2)];
  assert.ok(p50 < 5, `median wake-up lag ${p50.toFixed(2)} ms`);
  r.close();
  w.close();
});

test("waitForFrame rejects on a closed reader", () => {
  const name = uniqueName();
  const w = bridge.TestRingWriter.create(name, 4, 64, 16);
  const r = bridge.RingReader.open(name);
  r.close();
  assert.throws(() => r.waitForFrame(0, 10), /closed/);
  w.close();
});
