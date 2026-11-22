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
