// SPDX-License-Identifier: GPL-3.0-or-later
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  type BenchResult,
  buildResult,
  defaultResultPath,
  failed,
  frameStats,
  loadBudgets,
  metric,
  parseBudgets,
  quantile,
  readResult,
  table,
  verdict,
  writeResult,
} from "./result.ts";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");

const result = (metrics: BenchResult["metrics"], machine = "local"): BenchResult => ({
  machine,
  commit: "abc",
  date: "2026-11-28",
  suite: "ui",
  toolchain: { rustc: null, ffmpeg: null, wgpu: null, electron: "45.4.1" },
  metrics,
});

describe("the UI benchmarks' numbers", () => {
  it("reads every row of bench/budgets.toml, the UI rows among them", () => {
    const budgets = loadBudgets(root);
    const row = (id: string) => budgets.find((b) => b.id === id);
    expect(row("app_start_ms")).toMatchObject({ op: "<", value: 2500, unit: "ms" });
    expect(row("timeline_panzoom_5000_fps")).toMatchObject({ op: ">=", value: 55, unit: "fps" });
    expect(row("project_open_1h_ui_ms")).toMatchObject({ op: "<", value: 2000, unit: "ms" });
    expect(row("ui_command_rtt_p95_ms")).toMatchObject({ op: "<", value: 16, unit: "ms" });
    expect(row("timeline_panzoom_5000_frame_p95_ms")).toMatchObject({ op: "<", value: 25 });
    expect(row("playback_1080p60_dropped_frames")?.machine_scope).toEqual([
      "ref-win-igpu",
      "ref-mac-m1",
    ]);
    expect(new Set(budgets.map((b) => b.id)).size).toBe(budgets.length);
  });

  it("refuses a budget file it cannot read", () => {
    expect(() => parseBudgets('id = "x"')).toThrow(/cannot read/);
    expect(() => parseBudgets('[[wp]]\nid = "x"\nop = "<"')).toThrow(/incomplete row/);
  });

  it("judges a metric as cutweave-bench does", () => {
    const budgets = parseBudgets(
      [
        "# comment",
        "[[budget]]",
        'id = "fps"\nmetric = "m"\nop = ">="\nvalue = 55\nunit = "fps"\nmachine_scope = ["any"]',
        "[[wp]]",
        'id = "ms"\nmetric = "m"\nop = "<"\nvalue = 16\nunit = "ms"\nmachine_scope = ["ref-a"]',
      ].join("\n"),
    );
    expect(verdict(budgets, "local", metric("fps", 55, "fps", 9))).toBe("PASS");
    expect(verdict(budgets, "local", metric("fps", 54.9, "fps", 9))).toBe("FAIL");
    // A value in another unit fails, whatever it is.
    expect(verdict(budgets, "local", metric("fps", 60, "ms", 9))).toBe("FAIL");
    expect(verdict(budgets, "local", metric("ms", 99, "ms", 9))).toBe("skipped (other machines)");
    expect(verdict(budgets, "ref-a", metric("ms", 16, "ms", 9))).toBe("FAIL");
    expect(verdict(budgets, "ref-a", metric("ms", 15.9, "ms", 9))).toBe("PASS");
    expect(verdict(budgets, "local", metric("other", 1, "ms", 9))).toBe("info");

    const run = result([metric("fps", 40, "fps", 9), metric("other", 1, "ms", 1)]);
    expect(failed(budgets, run)).toEqual(["fps"]);
    const lines = table(budgets, run).trimEnd().split("\n");
    expect(lines).toHaveLength(3);
    expect(lines[1]).toMatch(/^fps\s+40\.000 fps\s+>= 55 fps\s+FAIL$/);
    expect(lines[2]).toMatch(/^other\s+1\.000 ms\s+-\s+info$/);
  });

  it("turns frame intervals into a rate and percentiles", () => {
    // 59 frames at 60 a second and one of 100 ms.
    const intervals = [...Array.from({ length: 59 }, () => 1000 / 60), 100];
    const stats = frameStats(intervals);
    expect(stats.frames).toBe(60);
    expect(stats.fps).toBeCloseTo(60_000 / (59 * (1000 / 60) + 100), 6);
    expect(stats.p50).toBeCloseTo(16.667, 2);
    expect(stats.p95).toBeCloseTo(16.667, 2);
    expect(stats.max).toBe(100);
    expect(Number.isNaN(frameStats([]).fps)).toBe(true);
    expect(quantile([5, 1, 3], 0.5)).toBe(3);
    expect(quantile([5, 1, 3], 1)).toBe(5);
    expect(Number.isNaN(quantile([], 0.95))).toBe(true);
  });

  it("writes a result where the gate looks, in the benchmark format", () => {
    const built = buildResult(root, "ui", [metric("app_start_ms", 1234.5678, "ms", 0)], "45.4.1", {
      gpu: "test",
    });
    expect(built.metrics).toEqual([
      { id: "app_start_ms", value: 1234.568, unit: "ms", samples: 1 },
    ]);
    expect(built.date).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    expect(built.toolchain.electron).toBe("45.4.1");
    expect(built.details).toMatchObject({ gpu: "test", logicalCpus: expect.any(Number) });
    expect(defaultResultPath("r", result([], "ref-win-igpu"))).toBe(
      path.join("r", "bench-results", "ref-win-igpu", "2026-11-28.ui.json"),
    );
    const dir = mkdtempSync(path.join(tmpdir(), "cutweave-perf-result-"));
    try {
      const file = path.join(dir, "deep", "ui.json");
      writeResult(file, built);
      expect(readResult(file)).toEqual(built);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
