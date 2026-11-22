// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1H.03 e2e: the cutweave-bridge addon loads in the main window's non-sandboxed preload and
// reads frames from a ring a separate process publishes (the Rust `ring_fixture`, through the
// engine's `FramePublisher`); a second, sandboxed window has no bridge.

import { expect, test } from "./fixtures.ts";
import { RingFixture } from "./ring-fixture.ts";

test.describe("preview bridge", () => {
  let fixture: RingFixture | null = null;
  test.afterEach(async () => {
    await fixture?.stop();
    fixture = null;
  });

  test("the addon loads in the main window and the page still has no Node access", async ({
    page,
  }) => {
    expect(await page.evaluate(() => window.cutweave.preview.isAvailable())).toBe(true);
    const globals = await page.evaluate(() => ({
      require: typeof (window as unknown as { require?: unknown }).require,
      process: typeof (window as unknown as { process?: unknown }).process,
      addon: typeof (window as unknown as { cutweaveBridge?: unknown }).cutweaveBridge,
      preview: Object.keys(window.cutweave.preview).sort(),
    }));
    expect(globals).toEqual({
      require: "undefined",
      process: "undefined",
      addon: "undefined",
      preview: [
        "close",
        "getClock",
        "getLatestFrame",
        "getPresentStats",
        "getStatus",
        "isAvailable",
        "open",
        "startPresenting",
        "stopPresenting",
        "waitForFrame",
      ],
    });
  });

  test("ring errors are typed", async ({ page }) => {
    fixture = await RingFixture.start(["--width", "64", "--height", "64"]);
    const attempt = (name: string, epoch: string) =>
      page.evaluate(
        ([n, e]) => {
          try {
            window.cutweave.preview.open(n as string, e as string);
            return "opened";
          } catch (err) {
            return String(err);
          }
        },
        [name, epoch],
      );
    expect(await attempt("cwp-00000001-0000000000000001", "1")).toContain("ERR_RING_NOT_FOUND");
    expect(await attempt("nope", "1")).toContain("ERR_RING_INVALID");
    const wrong = (BigInt(fixture.ready.epoch) ^ 1n).toString();
    expect(await attempt(fixture.ready.name, wrong)).toContain("ERR_RING_EPOCH");
    expect(await attempt(fixture.ready.name, fixture.ready.epoch)).toBe("opened");
    expect(await page.evaluate(() => window.cutweave.preview.getStatus().state)).toBe("live");
  });

  test("frames arrive in the main window, intact, and the preload loop presents them by clock", async ({
    page,
  }) => {
    fixture = await RingFixture.start(["--width", "1280", "--height", "720", "--fps", "60"]);
    const info = await page.evaluate(
      ([name, epoch]) => window.cutweave.preview.open(name as string, epoch as string),
      [fixture.ready.name, fixture.ready.epoch],
    );
    expect(info).toMatchObject({ name: fixture.ready.name, slotCount: 6 });

    // Pull path: a frame as the page API returns it (a clone across the context bridge).
    const frame = await page.evaluate(async () => {
      const preview = window.cutweave.preview;
      for (let i = 0; i < 100; i++) {
        await preview.waitForFrame(100);
        const f = preview.getLatestFrame();
        if (f) {
          const bytes = new Uint8Array(f.bytes);
          const stamp = new DataView(f.bytes).getUint32(0, true);
          const rows = [1, 100, 359, 719].map((row) => ({
            row,
            value: bytes[row * f.stride + 16],
          }));
          return {
            seq: f.seq,
            stamp,
            width: f.width,
            height: f.height,
            format: f.format,
            byteLength: f.bytes.byteLength,
            rows,
          };
        }
      }
      return null;
    });
    expect(frame).not.toBeNull();
    expect(frame).toMatchObject({ width: 1280, height: 720, format: "rgba8" });
    expect(frame?.byteLength).toBe(1280 * 720 * 4);
    expect(frame?.stamp).toBe((frame?.seq ?? 0) % 2 ** 32);
    for (const { row, value } of frame?.rows ?? []) {
      expect(value).toBe((((frame?.seq ?? 0) * 3 + row) & 0xff) >>> 0);
    }

    // Present loop: runs in the preload (rAF + clock-scheduled choice); only stats come back.
    await page.evaluate(() => window.cutweave.preview.startPresenting({ latencyMs: 20 }));
    await expect
      .poll(() => page.evaluate(() => window.cutweave.preview.getPresentStats()?.framesPresented), {
        timeout: 15_000,
      })
      .toBeGreaterThan(60);
    const stats = await page.evaluate(() => window.cutweave.preview.getPresentStats());
    expect(stats).toMatchObject({ running: true, lastWidth: 1280, lastHeight: 720, errors: 0 });
    expect(stats?.presentFps).toBeGreaterThan(30);
    const clock = await page.evaluate(() => window.cutweave.preview.getClock());
    expect(clock).toMatchObject({ state: "playing", rateMilli: 1000 });
    expect(clock.ptsFlicks).toBeGreaterThan(0);
    // The loop returns no pixels to the page, just numbers.
    expect(Object.keys(stats ?? {}).sort()).toEqual([
      "errors",
      "framesPresented",
      "lastError",
      "lastFormat",
      "lastHeight",
      "lastPts",
      "lastSeq",
      "lastWidth",
      "presentFps",
      "running",
      "seqGaps",
    ]);
    console.log(`[bridge e2e] present loop: ${JSON.stringify(stats)}`);
    await page.evaluate(() => window.cutweave.preview.stopPresenting());
  });

  test("a renegotiated (retired) ring is followed and a dead engine is detected", async ({
    page,
  }) => {
    fixture = await RingFixture.start(["--width", "320", "--height", "180", "--fps", "60"]);
    await page.evaluate(
      ([name, epoch]) => {
        window.cutweave.preview.open(name as string, epoch as string);
        window.cutweave.preview.startPresenting();
      },
      [fixture.ready.name, fixture.ready.epoch],
    );
    const presented = () =>
      page.evaluate(() => {
        const s = window.cutweave.preview.getPresentStats();
        return { n: s?.framesPresented ?? 0, w: s?.lastWidth ?? 0, seq: s?.lastSeq ?? 0 };
      });
    await expect.poll(async () => (await presented()).w).toBe(320);
    const before = await presented();
    fixture.send("resize 1024 576");
    await fixture.event("moved");
    await expect.poll(async () => (await presented()).w, { timeout: 10_000 }).toBe(1024);
    const after = await presented();
    expect(after.seq).toBeGreaterThan(before.seq);
    expect(await page.evaluate(() => window.cutweave.preview.getStatus().switches)).toBe(1);

    fixture.send("hang");
    await expect
      .poll(() => page.evaluate(() => window.cutweave.preview.getStatus().reason), {
        timeout: 5000,
      })
      .toBe("heartbeat-stale");
    expect(await page.evaluate(() => window.cutweave.preview.getStatus().alive)).toBe(false);
  });

  test("a second window is sandboxed and has no bridge", async ({ app }) => {
    const [aux] = await Promise.all([
      app.waitForEvent("window"),
      app.evaluate(() => globalThis.__cutweaveTest?.createWindow("auxiliary")),
    ]);
    await aux.waitForLoadState("domcontentloaded");
    expect(
      await aux.evaluate(() => ({
        cutweave: typeof (window as { cutweave?: unknown }).cutweave,
        require: typeof (window as { require?: unknown }).require,
        process: typeof (window as { process?: unknown }).process,
      })),
    ).toEqual({ cutweave: "undefined", require: "undefined", process: "undefined" });
  });
});
