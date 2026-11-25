// SPDX-License-Identifier: GPL-3.0-or-later
// What the M1 exit suite (WP-1O.04, docs/milestones/M1-exit.md) shares: the corpus clips of the
// cut, where the gate wants its artefacts, how many crash trials to run, and how a missing
// prerequisite is reported. `scripts/m1-gate.py` (`just m1-gate`) runs the suite with
// `CUTWEAVE_M1_OUT` set; run alone (`pnpm --filter @cutweave/desktop exec playwright test e2e/m1`)
// it keeps nothing and runs fewer trials.
import { cpSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "../fixtures.ts";

/** The three corpus clips of the M1 cut, in the order they are placed (WP-1O.04). */
export const M1_CLIPS = [
  "h264_1080p60.mp4",
  "hevc_10bit_sdr.mp4",
  "vfr_phone_rotated90.mp4",
] as const;

/** The project's name; the agent's project (`mcp_conformance/m1.rs`) has the same one. */
export const M1_PROJECT = "M1 cut";

/** The gate's artefact folder (`CUTWEAVE_M1_OUT`), or null when the suite runs alone. */
export const m1Out: string | null = process.env.CUTWEAVE_M1_OUT || null;

/**
 * Under the gate a missing prerequisite (an engine without FFmpeg, no corpus, no ffprobe) fails
 * the test, so the gate never reports a criterion it did not check. Run alone, the test skips and
 * says why.
 */
export function need(ok: boolean, why: string): void {
  if (ok) return;
  if (m1Out !== null) throw new Error(`M1 exit suite cannot run: ${why}`);
  test.skip(true, why);
}

/**
 * Crash trials of each kind: `CUTWEAVE_M1_TRIALS` (the gate passes 20, WP-1O.04), else `fallback`
 * (a quick run).
 */
export function trialCount(fallback: number): number {
  const n = Number.parseInt(process.env.CUTWEAVE_M1_TRIALS ?? "", 10);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

/** Writes `data` as JSON to `<CUTWEAVE_M1_OUT>/<rel>` (nothing when the suite runs alone). */
export function keepJson(rel: string, data: unknown): void {
  if (m1Out === null) return;
  const file = path.join(m1Out, rel);
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, `${JSON.stringify(data, null, 2)}\n`);
}

/**
 * Copies a file or folder to `<CUTWEAVE_M1_OUT>/<rel>`, replacing what a previous run left there
 * (nothing when the suite runs alone).
 */
export function keepCopy(from: string, rel: string): void {
  if (m1Out === null) return;
  const to = path.join(m1Out, rel);
  rmSync(to, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  mkdirSync(path.dirname(to), { recursive: true });
  cpSync(from, to, { recursive: true });
}
