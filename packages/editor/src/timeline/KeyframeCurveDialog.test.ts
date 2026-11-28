// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.05: what the curve editor's value becomes on the wire. A preset is sent by its name (so
// the document keeps the hint), anything else as its numbers; and the engine's own math samples
// the curve the dialog draws.
import type { Interp } from "@cutweave/protocol";
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { easing, initMath } from "@cutweave/protocol/wasm";
import type { CurvePreset, CurveValue } from "@cutweave/ui";
import { beforeAll, describe, expect, it } from "vitest";
import { easingInput } from "./KeyframeCurveDialog.tsx";

let presets: CurvePreset[] = [];

beforeAll(async () => {
  await initMath(mathModuleBytes());
  presets = easing
    .presets()
    .map((p) => ({ id: p.name, label: p.name, value: p.interp as CurveValue }));
}, 900_000);

describe("easingInput", () => {
  it("sends hold and linear by name", () => {
    expect(easingInput({ kind: "hold" }, presets)).toEqual({ easing: "hold" });
    expect(easingInput({ kind: "linear" }, presets)).toEqual({ easing: "linear" });
  });

  it("sends a preset's exact numbers by the preset's name", () => {
    const preset = presets.find((p) => p.id === "ease_in_out");
    expect(preset?.value).toMatchObject({ kind: "bezier", x1: 0.42, y2: 1 });
    expect(easingInput(preset?.value as CurveValue, presets)).toEqual({ easing: "ease_in_out" });
    const spring = presets.find((p) => p.id === "spring_wobbly");
    expect(easingInput(spring?.value as CurveValue, presets)).toEqual({ easing: "spring_wobbly" });
  });

  it("sends edited numbers as the stored form, without a stale preset hint", () => {
    const edited: CurveValue = { kind: "bezier", x1: 0.43, y1: 0, x2: 0.58, y2: 1 };
    expect(easingInput(edited, presets)).toEqual({ interp: edited });
    // A value that still carries the hint but no longer has the preset's numbers.
    const stale: CurveValue = { ...edited, preset: "ease_in_out" };
    expect(easingInput(stale, presets)).toEqual({ interp: edited });
  });
});

describe("the preview curve", () => {
  it("is the engine's own math: every preset starts at 0 and lands on 1", () => {
    expect(presets.length).toBeGreaterThanOrEqual(21);
    for (const preset of presets) {
      const ys = easing.sample(preset.value as Interp, 705_600_000, 64);
      expect(ys).toHaveLength(64);
      expect(ys[0], preset.id).toBeCloseTo(0, 6);
      if (preset.id !== "hold") expect(ys[63], preset.id).toBeCloseTo(1, 3);
    }
  });
});
