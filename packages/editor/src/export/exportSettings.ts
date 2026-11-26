// SPDX-License-Identifier: GPL-3.0-or-later
// What the export dialog (WP-1M.08) reads and checks besides the presets (`exportModel.ts`,
// WP-2G.04): the sequence's own size and rate, typed frame rates, the proposed file and its
// folder, and how the engine's progress line and encoder names read. Pure functions, so they are
// tested alone.

import type { ExportQuality, FrameRate, Sequence } from "@cutweave/protocol";

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

/** The folder a file path is in (no trailing separator, except a root); `null` for a bare name. */
export function dirName(path: string): string | null {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (cut < 0) return null;
  return cut === 0 || path[cut - 1] === ":" ? path.slice(0, cut + 1) : path.slice(0, cut);
}

/** `<folder>/<name>.mp4`, with the folder's own separator. */
export function pathInFolder(folder: string, name: string): string {
  const file = `${safeFileStem(name)}.mp4`;
  if (/[\\/]$/.test(folder)) return `${folder}${file}`;
  const sep = folder.includes("\\") && !folder.includes("/") ? "\\" : "/";
  return `${folder}${sep}${file}`;
}

/**
 * Where the dialog proposes to save: in the folder Settings > Export names (`mode`), or next to
 * the project when that folder is not known yet.
 */
export function proposedOutputPath(
  mode: "project" | "last" | "chosen",
  folders: { readonly last: string; readonly chosen: string },
  projectDoc: string | null,
  name: string,
): string {
  const folder = mode === "last" ? folders.last : mode === "chosen" ? folders.chosen : "";
  return folder === "" ? defaultOutputPath(projectDoc, name) : pathInFolder(folder, name);
}

/** Whether a path is one `export.start` takes: absolute. */
export function isAbsolutePath(path: string): boolean {
  return /^([a-zA-Z]:[\\/]|[\\/]{2}|\/)/.test(path.trim());
}

export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

// ---- encoders ----------------------------------------------------------------------------------

const ENCODER_FAMILIES: Readonly<Record<string, string>> = {
  nvenc: "NVIDIA NVENC",
  amf: "AMD AMF",
  qsv: "Intel Quick Sync",
  videotoolbox: "Apple VideoToolbox",
  vaapi: "VA-API",
};

/**
 * "NVIDIA NVENC" for `h264_nvenc`, `hevc_nvenc` or `av1_nvenc`; the implementation name itself
 * when its family is not known.
 */
export function encoderLabel(implementation: string): string {
  const family = /^(?:h264|hevc|av1)_([a-z]+)$/.exec(implementation)?.[1];
  return (family === undefined ? undefined : ENCODER_FAMILIES[family]) ?? implementation;
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
