// SPDX-License-Identifier: GPL-3.0-or-later
// The format new projects start with (#221, split from WP-2H.08). File > Project settings… >
// "Use as defaults for new projects" stores the project's format of new sequences
// (`settings.default_format`) in the app preference `editing.sequence_format`, and File > New
// project passes it to `project.new` as `settings.default_format`, so the first sequence and
// every later one start in it. The preference is one line of text,
// `1080x1920 30000/1001 44100 rec709 #000000` (width x height, frame rate as num/den, audio
// sample rate in Hz, color space, background), or `""` for "not set": then `project.new` gets no
// format and the engine's own applies (1920 × 1080, 30 fps, 48 kHz).
import { SEQUENCE_FORMAT_LINE } from "@cutweave/preferences";
import type { SequenceFormat } from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import type { TKey } from "../i18n/index.ts";

/** What `project.new` takes as `settings.default_format`. */
export interface NewSequenceFormat {
  readonly width: number;
  readonly height: number;
  readonly frame_rate: { readonly num: number; readonly den: number };
  readonly sample_rate: number;
  readonly color_space: string;
  readonly background: string;
}

// The engine's limits (`FormatSpec`): a line outside them reads as "not set", so a preferences
// file edited by hand can never keep File > New project from working.
const MAX_CANVAS_SIDE = 16384;
const MAX_FPS = 1000;
const MIN_SAMPLE_RATE = 8000;
const MAX_SAMPLE_RATE = 384000;

/** The format the preference holds, or `null` when it is not set (or is not a usable format). */
export function parseFormatPreference(text: string): NewSequenceFormat | null {
  const match = SEQUENCE_FORMAT_LINE.exec(text);
  if (match === null) return null;
  const [width, height, num, den, sampleRate] = match.slice(1, 6).map(Number) as [
    number,
    number,
    number,
    number,
    number,
  ];
  const side = (n: number): boolean => n >= 1 && n <= MAX_CANVAS_SIDE;
  if (!side(width) || !side(height)) return null;
  // 1 to 1000 fps and a whole number of flicks per frame (the engine's edit grid).
  if (num < 1 || den < 1 || num > MAX_FPS * den || num < den) return null;
  if ((FLICKS_PER_SECOND * den) % num !== 0) return null;
  if (sampleRate < MIN_SAMPLE_RATE || sampleRate > MAX_SAMPLE_RATE) return null;
  return {
    width,
    height,
    frame_rate: { num, den },
    sample_rate: sampleRate,
    color_space: match[6] as string,
    background: match[7] as string,
  };
}

/** A format for people: "1920 × 1080 at 29.97 fps". */
export function formatWords(
  format: { width: number; height: number; frame_rate: { num: number; den: number } },
  t: (key: TKey, vars?: Record<string, string>) => string,
): string {
  const rate = format.frame_rate;
  return t("projectSettings.formatValue", {
    width: String(format.width),
    height: String(format.height),
    fps: String(Math.round((rate.num / rate.den) * 1000) / 1000),
  });
}

/** The preference's line for `format`; `""` (not set) for a format that could not be read back. */
export function formatPreference(format: SequenceFormat): string {
  const { width, height, frame_rate: rate, sample_rate, color_space, background } = format;
  const text = `${width}x${height} ${rate.num}/${rate.den} ${sample_rate} ${color_space} ${String(
    background,
  ).toLowerCase()}`;
  return parseFormatPreference(text) === null ? "" : text;
}
