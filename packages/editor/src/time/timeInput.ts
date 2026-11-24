// SPDX-License-Identifier: GPL-3.0-or-later
// TypeScript port of the engine's `TimeInput` parser (arch §4.1, ADR-0015, `cutweave-model`
// `timecode.rs`): what a person types into a time field. Integer flicks (`"705600000"`), seconds
// (`"62.5s"`), a frame (`"f:1500"`) or a timecode (`"00:01:02:03"`, drop-frame `"00:01:02;03"`),
// resolved against the frame rate of the sequence that owns the clip. The Rust parser is the
// reference: `timeInput.test.ts` runs every case of `crates/cutweave-model/tests/fixtures/
// time-input.json`, which a Rust test generates. Exact BigInt math, so long decimals and
// frame * rate products never lose a flick.

import { FLICKS_PER_SECOND, type FrameRate } from "@cutweave/timeline";

/** Why an input was refused; the UI says "not a time" for `syntax`, the rest by reason. */
export type TimeInputError =
  /** Not one of the accepted forms. */
  | "syntax"
  /** A frame or timecode, but the frame rate is unknown. */
  | "needs_rate"
  /** Outside the safe range (|t| < 2^53 flicks). */
  | "range"
  /** A timecode that does not exist at this rate (frames past the rate, dropped label...). */
  | "invalid";

export type ParsedTime =
  | { readonly ok: true; readonly flicks: number }
  | { readonly ok: false; readonly error: TimeInputError };

const F = BigInt(FLICKS_PER_SECOND);
const MAX_SAFE = (1n << 53n) - 1n;
const MAX_SECONDS_DECIMALS = 18;
const U8_MAX = 255n;
const U32_MAX = 4_294_967_295n;

class Refused extends Error {
  constructor(readonly code: TimeInputError) {
    super(code);
  }
}

const fail = (code: TimeInputError): never => {
  throw new Refused(code);
};

/** A non-empty run of at most 30 ASCII digits, or `null`. */
function digits(s: string): bigint | null {
  if (s.length === 0 || s.length > 30 || !/^[0-9]+$/.test(s)) return null;
  return BigInt(s);
}

function safe(v: bigint): number {
  if (v > MAX_SAFE || v < -MAX_SAFE) return fail("range");
  return Number(v);
}

/** `ceil(a / b)` for positive `b`. */
function divCeil(a: bigint, b: bigint): bigint {
  return (a + b - 1n) / b;
}

/** Start of frame `n` (>= 0) at `rate`, rounded up to a whole flick as the engine does. */
function frameToFlicks(n: bigint, rate: FrameRate): number {
  return safe(divCeil(n * F * BigInt(rate.den), BigInt(rate.num)));
}

/** Whole frames per second of timecode labels: 30 for 29.97, 24 for 23.976. */
function nominal(rate: FrameRate): bigint {
  const n = (BigInt(rate.num) + BigInt(rate.den) / 2n) / BigInt(rate.den);
  return n < 1n ? 1n : n;
}

function dropFrames(rate: FrameRate): bigint | null {
  if (rate.den === 1001 && rate.num === 30_000) return 2n;
  if (rate.den === 1001 && rate.num === 60_000) return 4n;
  return null;
}

function parseTimecode(s: string, rate: FrameRate | null): number {
  const fields = s.split(/[:;]/);
  if (fields.length !== 4) return fail("syntax");
  const [h, m, sec, ff] = fields as [string, string, string, string];
  const drop = s[s.length - ff.length - 1] === ";";
  const num = (f: string): bigint => digits(f) ?? fail("syntax");
  const minutes = num(m);
  const seconds = num(sec);
  if (minutes > U8_MAX || seconds > U8_MAX) return fail("syntax");
  const hours = num(h);
  const frames = num(ff);
  if (hours > U32_MAX || frames > U32_MAX) return fail("range");
  if (minutes >= 60n || seconds >= 60n) return fail("invalid");
  if (rate === null) return fail("needs_rate");

  const base = nominal(rate);
  if (frames >= base) return fail("invalid");
  const totalMinutes = hours * 60n + minutes;
  let frame = (totalMinutes * 60n + seconds) * base + frames;
  if (drop) {
    const dropped = dropFrames(rate);
    if (dropped === null) return fail("invalid");
    // The labels `;00` .. `;<dropped-1>` do not exist at the start of every minute but each tenth.
    if (seconds === 0n && frames < dropped && minutes % 10n !== 0n) return fail("invalid");
    frame -= dropped * (totalMinutes - totalMinutes / 10n);
  }
  return frameToFlicks(frame, rate);
}

function parseAbsolute(s: string): number {
  const negative = s.startsWith("-");
  const body = negative ? s.slice(1) : s;
  let magnitude: bigint;
  if (body.endsWith("s")) {
    const secs = body.slice(0, -1);
    const dot = secs.indexOf(".");
    const whole = dot < 0 ? secs : secs.slice(0, dot);
    const frac = dot < 0 ? null : secs.slice(dot + 1);
    const wholeValue = digits(whole) ?? fail("syntax");
    magnitude = wholeValue * F;
    if (frac !== null) {
      if (frac.length > MAX_SECONDS_DECIMALS) return fail("syntax");
      const fracValue = digits(frac) ?? fail("syntax");
      const scale = 10n ** BigInt(frac.length);
      // Round half up on the magnitude: exact whenever the decimal is exact in flicks.
      magnitude += (2n * fracValue * F + scale) / (2n * scale);
    }
  } else if (body.length > 0 && /^[0-9]+$/.test(body)) {
    magnitude = digits(body) ?? fail("range");
  } else {
    return fail("syntax");
  }
  return safe(negative ? -magnitude : magnitude);
}

/**
 * Parses what a person typed. `rate` is the frame rate of the sequence that owns the clip;
 * `null` accepts only flicks and seconds (as the engine does for commands without a sequence).
 */
export function parseTimeInput(text: string, rate: FrameRate | null): ParsedTime {
  const s = text.trim();
  try {
    if (s.startsWith("f:")) {
      const n = digits(s.slice(2)) ?? fail("syntax");
      if (n > MAX_SAFE) return fail("range");
      if (rate === null) return fail("needs_rate");
      return { ok: true, flicks: frameToFlicks(n, rate) };
    }
    if (s.includes(":") || s.includes(";")) {
      return { ok: true, flicks: parseTimecode(s, rate) };
    }
    return { ok: true, flicks: parseAbsolute(s) };
  } catch (error) {
    if (error instanceof Refused) return { ok: false, error: error.code };
    throw error;
  }
}
