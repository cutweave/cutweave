// SPDX-License-Identifier: GPL-3.0-or-later
// Presenter logic (WP-1M.04): the playhead between `playback.state` events and the choice of
// frame by pts against that clock (arch §10 step 3: "the newest frame whose pts <= clock").
import { describe, expect, it } from "vitest";
import { type PlayheadSample, playheadAt, selectFrame, staleFrames } from "./presenter.ts";

const FLICKS = 705_600_000;
const F30 = FLICKS / 30;

const sample = (over: Partial<PlayheadSample> = {}): PlayheadSample => ({
  t: 10 * F30,
  rate: 1,
  playing: true,
  at: 1000,
  ...over,
});

describe("playheadAt", () => {
  it("advances at the rate from the sample's arrival", () => {
    expect(playheadAt(sample(), 1000)).toBe(10 * F30);
    expect(playheadAt(sample(), 1500)).toBe(10 * F30 + FLICKS / 2);
    expect(playheadAt(sample({ rate: 2 }), 1500)).toBe(10 * F30 + FLICKS);
    expect(playheadAt(sample({ rate: -1 }), 1100)).toBe(10 * F30 - FLICKS / 10);
  });

  it("holds while stopped and never goes below zero", () => {
    expect(playheadAt(sample({ playing: false }), 5000)).toBe(10 * F30);
    expect(playheadAt(sample({ rate: 0 }), 5000)).toBe(10 * F30);
    expect(playheadAt(sample({ rate: -4 }), 60_000)).toBe(0);
  });

  it("is an integer number of flicks", () => {
    expect(Number.isInteger(playheadAt(sample(), 1000 + 1 / 3))).toBe(true);
  });
});

const frames = (...pts: number[]) => pts.map((p) => ({ pts: p * F30, id: p }));

describe("selectFrame", () => {
  it("forward: the newest frame with pts <= clock - latency", () => {
    const buf = frames(10, 11, 12, 13);
    expect(selectFrame(buf, 12 * F30, { rate: 1 })?.id).toBe(12);
    expect(selectFrame(buf, 12 * F30 + F30 / 2, { rate: 1 })?.id).toBe(12);
    expect(selectFrame(buf, 13 * F30 - 1, { rate: 1 })?.id).toBe(12);
    // A 20 ms margin: frame 13 is due only 20 ms after its pts.
    const margin = (20 / 1000) * FLICKS;
    expect(selectFrame(buf, 13 * F30 + margin - 1, { rate: 1, latency: margin })?.id).toBe(12);
    expect(selectFrame(buf, 13 * F30 + margin, { rate: 1, latency: margin })?.id).toBe(13);
  });

  it("returns null while every frame is still in the future (keep showing the current one)", () => {
    expect(selectFrame(frames(11, 12), 10 * F30, { rate: 1 })).toBeNull();
    expect(selectFrame([], 10 * F30, { rate: 1 })).toBeNull();
  });

  it("does not depend on arrival order", () => {
    const buf = frames(13, 10, 12, 11);
    expect(selectFrame(buf, 12 * F30 + 5, { rate: 1 })?.id).toBe(12);
  });

  it("reverse: the frame containing clock + latency (time runs down)", () => {
    const buf = frames(10, 11, 12, 13);
    expect(selectFrame(buf, 12 * F30 + 5, { rate: -1 })?.id).toBe(12);
    const margin = (20 / 1000) * FLICKS;
    // Running backwards the margin points the other way: show the frame due 20 ms ago.
    expect(selectFrame(buf, 12 * F30 - margin, { rate: -1, latency: margin })?.id).toBe(12);
    expect(selectFrame(buf, 12 * F30 - margin - 1, { rate: -1, latency: margin })?.id).toBe(11);
  });

  it("stopped: the frame nearest the playhead, whatever the latency", () => {
    const buf = frames(10, 14);
    expect(selectFrame(buf, 13 * F30, { rate: 0, latency: F30 })?.id).toBe(14);
    expect(selectFrame(buf, 11 * F30, { rate: 0 })?.id).toBe(10);
  });
});

describe("staleFrames", () => {
  it("forward: frames older than the shown one; reverse: newer ones", () => {
    const buf = frames(10, 11, 12, 13);
    const shown = buf[2] ?? null;
    expect(staleFrames(buf, shown, 1).map((f) => f.id)).toEqual([10, 11]);
    expect(staleFrames(buf, shown, -1).map((f) => f.id)).toEqual([13]);
    expect(staleFrames(buf, null, 1)).toEqual([]);
  });
});
