// SPDX-License-Identifier: GPL-3.0-or-later
/** Pure number helpers for NumberScrubber. Integer steps never accumulate float drift. */

/** Number of decimal places in the shortest decimal representation of `n`. */
export function decimalsOf(n: number): number {
  if (!Number.isFinite(n)) return 0;
  const text = String(n);
  const exp = /e([+-]\d+)$/.exec(text);
  const mantissa = exp ? text.slice(0, exp.index) : text;
  const fraction = mantissa.split(".")[1]?.length ?? 0;
  return Math.max(0, fraction - (exp ? Number(exp[1]) : 0));
}

/** Rounds to `places` decimals through a decimal string, so 0.1 + 0.2 becomes 0.3. */
export function roundTo(n: number, places: number): number {
  return Number(n.toFixed(Math.min(Math.max(places, 0), 20)));
}

export function clamp(n: number, min: number, max: number): number {
  return Math.min(Math.max(n, min), max);
}

export type StepModifiers = { shift?: boolean; alt?: boolean };

/** Step multiplier: Shift x10, Alt x0.1, both cancel out. */
export function modifierScale({ shift, alt }: StepModifiers): number {
  if (shift && !alt) return 10;
  if (alt && !shift) return 0.1;
  return 1;
}

/**
 * Moves `value` by `count` steps of `step * scale` and clamps to the range. The
 * result is rounded to the finest precision involved, so integer steps stay
 * exact integers and fine steps stay clean decimals.
 */
export function stepValue(
  value: number,
  count: number,
  step: number,
  scale: number,
  min: number,
  max: number,
): number {
  const delta = step * scale;
  const places = Math.max(decimalsOf(value), decimalsOf(step), decimalsOf(delta));
  return clamp(roundTo(value + count * delta, places), min, max);
}

/** Parses typed input, tolerating a trailing unit and a decimal comma. Returns NaN if invalid. */
export function parseNumber(text: string, unit?: string): number {
  let t = text.trim();
  if (unit && t.endsWith(unit)) t = t.slice(0, -unit.length).trim();
  t = t.replace(",", ".");
  if (t === "") return Number.NaN;
  const n = Number(t);
  return Number.isFinite(n) ? n : Number.NaN;
}

/**
 * Parses what a person typed into a number field, where `current` is the value shown. Besides a
 * plain number it takes a relative entry: `+10` adds, `*2` multiplies, `/2` divides, and `50%`
 * is half of the current value, unless the field's own unit is `%` (then `50%` is just 50). A
 * leading `-` stays a negative number. Returns NaN when it is none of these.
 */
export function parseEntry(text: string, current: number, unit?: string): number {
  const t = text.trim();
  const op = t[0];
  if (op === "+" || op === "*" || op === "/") {
    const operand = parseNumber(t.slice(1), unit);
    if (Number.isNaN(operand) || !Number.isFinite(current)) return Number.NaN;
    if (op === "+") return roundTo(current + operand, relativePlaces(current, operand));
    if (op === "*") return roundTo(current * operand, relativePlaces(current, operand));
    return operand === 0 ? Number.NaN : roundTo(current / operand, 6);
  }
  if (unit !== "%" && t.endsWith("%")) {
    const percent = parseNumber(t.slice(0, -1));
    if (Number.isNaN(percent) || !Number.isFinite(current)) return Number.NaN;
    return roundTo((current * percent) / 100, 6);
  }
  return parseNumber(t, unit);
}

/** Decimals that keep a sum or product of two shown numbers exact. */
function relativePlaces(a: number, b: number): number {
  return Math.min(decimalsOf(a) + decimalsOf(b), 10);
}
