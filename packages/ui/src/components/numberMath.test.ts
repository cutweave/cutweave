// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import {
  clamp,
  decimalsOf,
  modifierScale,
  parseEntry,
  parseNumber,
  roundTo,
  stepValue,
} from "./numberMath.ts";

describe("numberMath", () => {
  it("counts decimals, including exponent forms", () => {
    expect(decimalsOf(5)).toBe(0);
    expect(decimalsOf(0.25)).toBe(2);
    expect(decimalsOf(1e-7)).toBe(7);
    expect(decimalsOf(1.5e-7)).toBe(8);
    expect(decimalsOf(Number.NaN)).toBe(0);
  });

  it("rounds through decimals so sums stay clean", () => {
    expect(0.1 + 0.2).not.toBe(0.3);
    expect(roundTo(0.1 + 0.2, 1)).toBe(0.3);
    expect(clamp(5, 0, 3)).toBe(3);
  });

  it("maps modifiers to scales", () => {
    expect(modifierScale({})).toBe(1);
    expect(modifierScale({ shift: true })).toBe(10);
    expect(modifierScale({ alt: true })).toBe(0.1);
    expect(modifierScale({ shift: true, alt: true })).toBe(1);
  });

  it("steps integers exactly over many iterations", () => {
    let v = 0;
    for (let i = 0; i < 1000; i++) v = stepValue(v, 1, 1, 1, -Infinity, Infinity);
    expect(v).toBe(1000);
    expect(Number.isInteger(v)).toBe(true);
  });

  it("steps x10 and x0.1 without drift", () => {
    expect(stepValue(100, 1, 1, 10, -Infinity, Infinity)).toBe(110);
    expect(stepValue(0.1, 1, 1, 0.1, -Infinity, Infinity)).toBe(0.2);
    let v = 0;
    for (let i = 0; i < 30; i++) v = stepValue(v, 1, 1, 0.1, -Infinity, Infinity);
    expect(v).toBe(3);
    expect(stepValue(0.2, 1, 0.1, 1, -Infinity, Infinity)).toBe(0.3);
  });

  it("clamps to the range", () => {
    expect(stepValue(99, 5, 1, 1, 0, 100)).toBe(100);
    expect(stepValue(1, -5, 1, 1, 0, 100)).toBe(0);
  });

  it("parses typed input with units and decimal commas", () => {
    expect(parseNumber(" 12.5 dB", "dB")).toBe(12.5);
    expect(parseNumber("1,5")).toBe(1.5);
    expect(parseNumber("")).toBeNaN();
    expect(parseNumber("abc")).toBeNaN();
    expect(parseNumber("Infinity")).toBeNaN();
  });

  it("reads relative entries against the shown value", () => {
    expect(parseEntry("+10", 25)).toBe(35);
    expect(parseEntry("+0.1", 0.2)).toBe(0.3);
    expect(parseEntry("*2", 7.5)).toBe(15);
    expect(parseEntry("/4", 10)).toBe(2.5);
    expect(parseEntry("50%", 80)).toBe(40);
    expect(parseEntry("+5 dB", -6, "dB")).toBe(-1);
    // A field whose own unit is % takes 50% as 50.
    expect(parseEntry("50%", 80, "%")).toBe(50);
    expect(parseEntry("+10%", 20, "%")).toBe(30);
    // Plain numbers and a leading minus stay absolute.
    expect(parseEntry("-10", 5)).toBe(-10);
    expect(parseEntry("12,5", 5)).toBe(12.5);
    expect(parseEntry("/0", 5)).toBeNaN();
    expect(parseEntry("*", 5)).toBeNaN();
    expect(parseEntry("two", 5)).toBeNaN();
  });
});
