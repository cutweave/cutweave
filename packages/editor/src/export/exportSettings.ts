// SPDX-License-Identifier: GPL-3.0-or-later
// What the export dialog (WP-1M.08) decides before it calls `export.start`: the defaults taken
// from the sequence, the checks on what a person typed, the request the settings make, and how
// the engine's progress line and encoder names read. Pure functions, so they are tested alone.
//
// Contract: `export.start` (arch §7 "Export", §6): `preset` is "h264-mp4-1080p" or
// `{custom: {width, height, fps, quality}}`; `hw` is "auto", "off" or `{force: name}`.

import type { ExportHw, ExportQuality, ExportStart, FrameRate, Sequence } from "@cutweave/protocol";

export const QUALITIES: readonly ExportQuality[] = ["low", "medium", "high"];

/** The sequence's own canvas and rate, as the dialog shows them. */
export interface SequenceFacts {
  readonly width: number;
  readonly height: number;
  readonly fps: FrameRate;
}

export function sequenceFacts(sequence: Sequence): SequenceFacts {
  const { width, height, frame_rate } = sequence.format;
  return { width, height, fps: frame_rate };
}

const even = (n: number): number => Math.max(2, Math.floor(n / 2) * 2);

/**
 * The size the "MP4 H.264 1080p" preset writes: the canvas fit into 1920x1080 (1080x1920 when
 * portrait), never upscaled, with even sides.
 */
export function presetSize(facts: Pick<SequenceFacts, "width" | "height">): {
  width: number;
  height: number;
} {
  const [boxW, boxH] = facts.height > facts.width ? [1080, 1920] : [1920, 1080];
  const scale = Math.min(1, boxW / facts.width, boxH / facts.height);
  return { width: even(facts.width * scale), height: even(facts.height * scale) };
}

/** A frame rate as people write it: "30", "29.97", "23.976". */
export function fpsText(rate: FrameRate): string {
  const value = rate.num / rate.den;
  return Number.isInteger(value) ? String(value) : String(Math.round(value * 1000) / 1000);
}

const NTSC: readonly FrameRate[] = [
  { num: 24000, den: 1001 },
  { num: 30000, den: 1001 },
  { num: 60000, den: 1001 },
  { num: 120000, den: 1001 },
];

/** Reads typed frames per second into a rate; `null` when it is not a rate the engine takes. */
export function parseFps(text: string): FrameRate | null {
  const value = Number(text.trim().replace(",", "."));
  if (!Number.isFinite(value) || value <= 0 || value > 240) return null;
  if (Number.isInteger(value)) return { num: value, den: 1 };
  const ntsc = NTSC.find((r) => Math.abs(r.num / r.den - value) < 0.0015);
  if (ntsc !== undefined) return ntsc;
  const thousandths = Math.round(value * 1000);
  return thousandths < 1 ? null : { num: thousandths, den: 1000 };
}

export const MIN_SIDE = 16;
export const MAX_SIDE = 8192;

/** What the person typed for the size, rate and quality. */
export interface FormatDraft {
  readonly width: string;
  readonly height: string;
  readonly fps: string;
  readonly quality: ExportQuality;
}

/** The draft that means "the preset": the fitted size, the sequence rate, high quality. */
export function defaultDraft(facts: SequenceFacts): FormatDraft {
  const size = presetSize(facts);
  return {
    width: String(size.width),
    height: String(size.height),
    fps: fpsText(facts.fps),
    quality: "high",
  };
}

export type FormatProblem = "width" | "height" | "fps";

const wholeNumber = (text: string): number | null => {
  const trimmed = text.trim();
  return /^\d+$/.test(trimmed) ? Number(trimmed) : null;
};

const sideOk = (n: number | null): n is number =>
  n !== null && n >= MIN_SIDE && n <= MAX_SIDE && n % 2 === 0;

/** The first field the engine would refuse (even sides of 16 to 8192, at most 240 fps). */
export function formatProblem(draft: FormatDraft): FormatProblem | null {
  if (!sideOk(wholeNumber(draft.width))) return "width";
  if (!sideOk(wholeNumber(draft.height))) return "height";
  if (parseFps(draft.fps) === null) return "fps";
  return null;
}

/** The `preset` of the request: the named preset when nothing differs from it, else custom. */
export function presetFor(draft: FormatDraft, facts: SequenceFacts): ExportStart["preset"] {
  const base = defaultDraft(facts);
  const rate = parseFps(draft.fps);
  const same =
    draft.width.trim() === base.width &&
    draft.height.trim() === base.height &&
    draft.quality === base.quality &&
    rate !== null &&
    rate.num * facts.fps.den === facts.fps.num * rate.den;
  if (same) return "h264-mp4-1080p";
  return {
    custom: {
      width: wholeNumber(draft.width),
      height: wholeNumber(draft.height),
      fps: rate,
      quality: draft.quality,
    },
  };
}

/** "Auto" when the switch is on, else software only. */
export function hwFor(useHardware: boolean): ExportHw {
  return useHardware ? "auto" : "off";
}

// ---- the file name -----------------------------------------------------------------------------

/** A project name made safe to be a file name. */
export function safeFileStem(name: string): string {
  const stem = Array.from(name, (c) => (c.charCodeAt(0) < 32 || '<>:"/\\|?*'.includes(c) ? " " : c))
    .join("")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/[. ]+$/, "");
  return stem === "" ? "Export" : stem;
}

/** `<project folder>/<name>.mp4`; just the file name when the project has no folder yet. */
export function defaultOutputPath(projectDoc: string | null, name: string): string {
  const file = `${safeFileStem(name)}.mp4`;
  if (projectDoc === null) return file;
  const cut = Math.max(projectDoc.lastIndexOf("/"), projectDoc.lastIndexOf("\\"));
  if (cut < 0) return file;
  const sep = projectDoc[cut] ?? "/";
  return `${projectDoc.slice(0, cut)}${sep}${file}`;
}

/** Whether a path is one `export.start` takes: absolute. */
export function isAbsolutePath(path: string): boolean {
  return /^([a-zA-Z]:[\\/]|[\\/]{2}|\/)/.test(path.trim());
}

export function hasMp4Extension(path: string): boolean {
  return /\.mp4$/i.test(path.trim());
}

export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

// ---- encoders ----------------------------------------------------------------------------------

const ENCODER_NAMES: Readonly<Record<string, string>> = {
  h264_nvenc: "NVIDIA NVENC",
  h264_amf: "AMD AMF",
  h264_qsv: "Intel Quick Sync",
  h264_videotoolbox: "Apple VideoToolbox",
  h264_vaapi: "VA-API",
};

/** "NVIDIA NVENC" for `h264_nvenc`; the implementation name itself when it is not known. */
export function encoderLabel(implementation: string): string {
  return ENCODER_NAMES[implementation] ?? implementation;
}

/**
 * The hardware H.264 encoders in a `media.capabilities` answer (`hw_encode[].implementation`,
 * in the order `auto` tries them), or `null` when the answer is not that shape.
 */
export function hardwareEncoders(answer: unknown): string[] | null {
  if (typeof answer !== "object" || answer === null) return null;
  const list = (answer as { hw_encode?: unknown }).hw_encode;
  if (!Array.isArray(list)) return null;
  const names: string[] = [];
  for (const entry of list) {
    const name = (entry as { implementation?: unknown } | null)?.implementation;
    if (typeof name === "string" && !names.includes(name)) names.push(name);
  }
  return names;
}

// ---- progress ----------------------------------------------------------------------------------

/** What the engine's progress line ("frame 120 of 600, 31.2 fps, about 0:15 left") says. */
export interface ProgressLine {
  readonly fps: string | null;
  readonly eta: string | null;
  /** A hardware encoder failed and the export began again in software. */
  readonly restartedInSoftware: boolean;
}

export function parseProgressLine(message: string | null): ProgressLine {
  const text = message ?? "";
  return {
    fps: /(\d+(?:\.\d+)?)\s*fps/i.exec(text)?.[1] ?? null,
    eta: /about\s+(\d+:\d{2}(?::\d{2})?)\s+left/i.exec(text)?.[1] ?? null,
    restartedInSoftware: /again in software/i.test(text),
  };
}

/** Percent 0-100 from the job's fraction. */
export function percent(fraction: number | null): number {
  if (fraction === null || !Number.isFinite(fraction)) return 0;
  return Math.min(100, Math.max(0, Math.round(fraction * 100)));
}
