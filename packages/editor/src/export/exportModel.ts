// SPDX-License-Identifier: GPL-3.0-or-later
// What the export dialog (WP-2G.04) works out before it asks the engine: the preset picker's
// groups (presets that fit the sequence first, then each platform, then the general formats and
// your own), the summary line, the estimated size, the warnings in words, the range and how many
// files it makes, and the file name. Pure functions over protocol types, tested alone.
//
// Contracts: `export.list_presets`, `export.resolve_preset`, `export.start`/`export.enqueue`
// (`range`, `collision`, `advanced`), docs/reference/export-ranges.md and export-quality.md.

import type {
  AudioCodec,
  Collision,
  Container,
  CustomExport,
  ExportPreset,
  ExportPresetEntry,
  ExportPresetSpec,
  ExportQuality,
  ExportRange,
  FrameRate,
  MarkerId,
  PairMode,
  PresetWarning,
  Sequence,
  VideoCodec,
} from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import type { TKey } from "../i18n/index.ts";
import { parseTimeInput } from "../time/timeInput.ts";
import { MAX_SIDE, MIN_SIDE, parseFps, safeFileStem } from "./exportSettings.ts";

/** The general-purpose preset the dialog starts at when nothing else is known. */
export const GENERAL_PRESET = "h264-mp4-1080p";

// ---- aspect ------------------------------------------------------------------------------------

const gcd = (a: number, b: number): number => (b === 0 ? a : gcd(b, a % b));

/** `16:9`, `9:16`, `4:5` (the engine's `ratio`). */
export function ratioText(width: number, height: number): string {
  const g = Math.max(1, gcd(Math.round(width), Math.round(height)));
  return `${Math.round(width) / g}:${Math.round(height) / g}`;
}

/** The engine's `same_aspect`: within 1 %. */
export function sameAspect(aw: number, ah: number, bw: number, bh: number): boolean {
  const x = aw * bh;
  const y = ah * bw;
  return Math.abs(x - y) * 100 <= Math.max(x, y);
}

/** The size a preset always writes, or `null` when it follows or fits the sequence. */
export function fixedSize(spec: ExportPresetSpec): { width: number; height: number } | null {
  return spec.size.kind === "fixed" ? { width: spec.size.width, height: spec.size.height } : null;
}

/** A platform preset made for a sequence of this shape. */
export function fitsSequence(
  spec: ExportPresetSpec,
  seq: { readonly width: number; readonly height: number },
): boolean {
  const size = fixedSize(spec);
  return size !== null && sameAspect(seq.width, seq.height, size.width, size.height);
}

// ---- the picker --------------------------------------------------------------------------------

/** One row of the picker. */
export interface PresetOption {
  readonly id: string;
  readonly name: string;
  readonly summary: string;
  readonly supported: boolean;
  readonly reason: string | null;
}

/** A heading of the picker: translated (`key`) or a platform's own name (`name`). */
export type GroupLabel =
  | { readonly key: TKey; readonly vars?: Readonly<Record<string, string>> }
  | { readonly name: string };

export interface PresetGroup {
  readonly id: string;
  readonly label: GroupLabel;
  readonly options: readonly PresetOption[];
}

const option = (entry: ExportPresetEntry): PresetOption => ({
  id: entry.preset.id,
  name: entry.preset.name,
  summary: entry.preset.summary ?? "",
  supported: entry.supported,
  reason: entry.unsupported_reason ?? null,
});

/** The platform a preset belongs to (`youtube` for YouTube Shorts too), or `null`. */
export function brandOf(spec: ExportPresetSpec): string | null {
  const id = spec.platform?.id;
  if (id === undefined || id === "") return null;
  return id.split("-")[0] ?? id;
}

/** The name of a platform: the preset platform named exactly so, else its first word. */
function brandName(brand: string, entries: readonly ExportPresetEntry[]): string {
  const exact = entries.find((e) => e.preset.platform?.id === brand)?.preset.platform?.name;
  if (exact !== undefined) return exact;
  const first = entries.find((e) => brandOf(e.preset) === brand)?.preset.platform?.name ?? brand;
  return first.split(" ")[0] ?? first;
}

/**
 * The picker's groups. Platform presets made for the sequence's shape come first (on a 9:16
 * sequence: Shorts, Reels, Stories, TikTok, X portrait); then the other platform presets under
 * each platform, in the pack's order; then the general formats; then your own.
 */
export function groupPresets(
  entries: readonly ExportPresetEntry[],
  seq: { readonly width: number; readonly height: number },
): PresetGroup[] {
  const builtin = entries.filter((e) => e.source === "builtin");
  const platform = builtin.filter((e) => brandOf(e.preset) !== null);
  const groups: PresetGroup[] = [];
  const matching = platform.filter((e) => fitsSequence(e.preset, seq));
  if (matching.length > 0) {
    groups.push({
      id: "match",
      label: {
        key: "export.presets.match",
        vars: { ratio: ratioText(seq.width, seq.height) },
      },
      options: matching.map(option),
    });
  }
  const brands: string[] = [];
  for (const e of platform) {
    const brand = brandOf(e.preset) as string;
    if (!brands.includes(brand)) brands.push(brand);
  }
  for (const brand of brands) {
    const rest = platform.filter((e) => brandOf(e.preset) === brand && !matching.includes(e));
    if (rest.length > 0) {
      groups.push({
        id: `brand:${brand}`,
        label: { name: brandName(brand, platform) },
        options: rest.map(option),
      });
    }
  }
  const general = builtin.filter((e) => brandOf(e.preset) === null);
  if (general.length > 0) {
    groups.push({
      id: "general",
      label: { key: "export.presets.general" },
      options: general.map(option),
    });
  }
  const mine = entries.filter((e) => e.source === "user");
  if (mine.length > 0) {
    groups.push({ id: "user", label: { key: "export.presets.user" }, options: mine.map(option) });
  }
  return groups;
}

/** The preset a new dialog starts at: the first of `wanted` that exists and works, else the general one. */
export function startingPreset(
  entries: readonly ExportPresetEntry[],
  wanted: readonly string[],
): string {
  for (const id of [...wanted, GENERAL_PRESET]) {
    if (id !== "" && entries.some((e) => e.preset.id === id && e.supported)) return id;
  }
  return entries.find((e) => e.supported)?.preset.id ?? GENERAL_PRESET;
}

// ---- the summary -------------------------------------------------------------------------------

const CODEC_LABEL: Readonly<Record<VideoCodec, string>> = {
  h264: "H.264",
  hevc: "HEVC",
  av1: "AV1",
  prores: "ProRes",
  gif: "GIF",
};
const AUDIO_LABEL: Readonly<Record<AudioCodec, string>> = {
  aac: "AAC",
  mp3: "MP3",
  pcm: "PCM",
  flac: "FLAC",
  opus: "Opus",
};
const CONTAINER_LABEL: Readonly<Record<Container, string>> = {
  mp4: "MP4",
  mov: "MOV",
  gif: "GIF",
  wav: "WAV",
  mp3: "MP3",
  m4a: "M4A",
  flac: "FLAC",
  ogg: "Ogg",
};

/** `H.264 + AAC`, `ProRes + PCM`, `GIF`, `AAC` (audio only). */
export function codecsText(spec: ExportPresetSpec): string {
  const video =
    spec.video === null || spec.video === undefined ? null : CODEC_LABEL[spec.video.codec];
  const audio =
    spec.audio === null || spec.audio === undefined ? null : AUDIO_LABEL[spec.audio.codec];
  return [video, audio].filter((p): p is string => p !== null).join(" + ");
}

export function containerText(container: Container): string {
  return CONTAINER_LABEL[container];
}

/** The file's extension, with the dot: `.mp4`, `.mov`. */
export function containerExtension(container: Container): string {
  return `.${container}`;
}

/** A frame rate as people write it: `30`, `29.97`. */
export function fpsLabel(rate: FrameRate): string {
  const value = rate.num / rate.den;
  return Number.isInteger(value) ? String(value) : String(Math.round(value * 100) / 100);
}

/** `n` bytes as `512 MB` or `3.4 GB` (decimal units, as the engine's disk check says it). */
export function humanBytes(n: number): string {
  const MB = 1_000_000;
  const GB = 1_000_000_000;
  if (n >= GB) return `${Math.floor(n / GB)}.${Math.floor((n % GB) / (GB / 10))} GB`;
  return `${Math.max(1, Math.ceil(n / MB))} MB`;
}

/** Seconds as `m:ss` (`h:mm:ss` from an hour). */
export function clockText(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** The x264 quality each choice means (the engine's mapping of `quality`). */
export const QUALITY_CRF: Readonly<Record<ExportQuality, number>> = { low: 28, medium: 23, high: 20 };

/** The quality choice nearest a preset's CRF: 20 and below High, to 23 Medium, else Low. */
export function qualityOfCrf(crf: number): ExportQuality {
  return crf <= 20 ? "high" : crf <= 23 ? "medium" : "low";
}

/** Typical bits per pixel (thousandths) of x264 at a CRF: an upper-end figure (the engine's). */
function bppMilli(crf: number): number {
  if (crf <= 16) return 200;
  if (crf <= 20) return 150;
  if (crf <= 23) return 100;
  if (crf <= 28) return 50;
  return 25;
}

export interface SizeEstimate {
  readonly bytes: number;
  /** A quality (CRF) export has no fixed bitrate: this is an upper-end guess. */
  readonly upTo: boolean;
}

/**
 * The file's size: the engine's figure when the preset fixes a bitrate (`resolvedKib`, bitrate
 * times length), else the engine's typical bitrate for the quality, as an upper-end guess.
 */
export function estimateSize(input: {
  readonly spec: ExportPresetSpec;
  readonly width: number;
  readonly height: number;
  readonly fps: FrameRate;
  readonly seconds: number;
  readonly quality: ExportQuality | null;
  readonly resolvedKib: number | null;
}): SizeEstimate | null {
  const { spec, seconds } = input;
  if (!(seconds > 0)) return null;
  if (input.quality === null && input.resolvedKib !== null) {
    return { bytes: input.resolvedKib * 1024, upTo: false };
  }
  const rate = spec.video?.rate_control;
  if (rate === undefined || rate.mode === "profile") return null;
  const audioBps = (spec.audio?.bitrate_kbps ?? 192) * 1000;
  let videoBps: number;
  if (input.quality === null && (rate.mode === "vbr" || rate.mode === "cbr")) {
    videoBps = rate.bitrate_kbps * 1000;
  } else {
    const crf =
      input.quality !== null ? QUALITY_CRF[input.quality] : rate.mode === "crf" ? rate.crf : 23;
    videoBps =
      (input.width * input.height * (input.fps.num / input.fps.den) * bppMilli(crf)) / 1000;
  }
  const upTo = input.quality !== null || rate.mode === "crf";
  return { bytes: Math.ceil((((videoBps + audioBps) * seconds) / 8) * 1.05), upTo };
}

// ---- warnings in words -------------------------------------------------------------------------

export interface WarningText {
  readonly code: string;
  readonly key: TKey | null;
  readonly vars: Readonly<Record<string, string>>;
  /** The engine's own sentence, for a code this build does not know. */
  readonly message: string;
}

/** What a preset warning says, from what the dialog knows (the engine's sentence as fallback). */
export function warningText(
  warning: PresetWarning,
  ctx: {
    readonly spec: ExportPresetSpec;
    readonly seq: { readonly width: number; readonly height: number };
    readonly width: number;
    readonly height: number;
    readonly fps: FrameRate;
    readonly seconds: number;
    readonly estimate: SizeEstimate | null;
  },
): WarningText {
  const target = ctx.spec.platform?.name ?? ctx.spec.name;
  const base = { code: warning.code, message: warning.message };
  switch (warning.code) {
    case "AspectMismatch": {
      const size = fixedSize(ctx.spec);
      return {
        ...base,
        key: "export.warnings.aspect",
        vars: {
          preset: ctx.spec.name,
          presetRatio: size === null ? "" : ratioText(size.width, size.height),
          seqRatio: ratioText(ctx.seq.width, ctx.seq.height),
          width: String(ctx.width),
          height: String(ctx.height),
        },
      };
    }
    case "DurationOverLimit":
      return {
        ...base,
        key: "export.warnings.duration",
        vars: {
          platform: target,
          limit: clockText(ctx.spec.limits?.max_duration_secs ?? 0),
          length: clockText(ctx.seconds),
        },
      };
    case "SizeOverLimit":
      return {
        ...base,
        key: "export.warnings.size",
        vars: {
          platform: target,
          size: ctx.estimate === null ? "" : humanBytes(ctx.estimate.bytes),
          limit: humanBytes((ctx.spec.limits?.max_size_mb ?? 0) * 1024 * 1024),
        },
      };
    case "FrameRateCapped": {
      const max = ctx.spec.fps.kind === "follow_sequence" ? ctx.spec.fps.max : null;
      return {
        ...base,
        key: "export.warnings.fps",
        vars: {
          platform: target,
          max: max === null || max === undefined ? fpsLabel(ctx.fps) : fpsLabel(max),
          fps: fpsLabel(ctx.fps),
        },
      };
    }
    case "SoftwareEncoder":
      return { ...base, key: "export.warnings.software", vars: {} };
    default:
      return { ...base, key: null, vars: {} };
  }
}

// ---- the range ---------------------------------------------------------------------------------

export const RANGE_KINDS = ["sequence", "in_out", "custom", "markers"] as const;
export type RangeKind = (typeof RANGE_KINDS)[number];

export interface RangeDraft {
  readonly kind: RangeKind;
  /** Custom start and end, as typed (timecode, seconds or `f:` frames). */
  readonly start: string;
  readonly end: string;
  readonly pairMode: PairMode;
}

export const DEFAULT_RANGE: RangeDraft = {
  kind: "sequence",
  start: "",
  end: "",
  pairMode: "duration",
};

/** The end of the last item: the sequence's length. */
export function sequenceLength(seq: Sequence): number {
  let end = 0;
  for (const track of Object.values(seq.tracks)) {
    for (const item of Object.values(track.items)) end = Math.max(end, item.start + item.duration);
  }
  return end;
}

/** One file of a marker set: where it starts and ends, and its label. */
export interface MarkerRegion {
  readonly id: MarkerId;
  readonly label: string;
  readonly start: number;
  readonly end: number;
}

/**
 * The files a marker set makes (the engine's rule): each marker's own duration (markers without
 * one are skipped), or each marker to the next. Regions shorter than a flick are dropped.
 */
export function markerRegions(seq: Sequence, pairMode: PairMode): MarkerRegion[] {
  const markers = Object.values(seq.markers ?? {}).sort((a, b) => a.t - b.t);
  const out: MarkerRegion[] = [];
  if (pairMode === "pairs") {
    for (let i = 0; i + 1 < markers.length; i++) {
      const a = markers[i];
      const b = markers[i + 1];
      if (a !== undefined && b !== undefined && b.t > a.t) {
        out.push({ id: a.id, label: a.label, start: a.t, end: b.t });
      }
    }
  } else {
    for (const m of markers) {
      if (m.duration !== null && m.duration > 0) {
        out.push({ id: m.id, label: m.label, start: m.t, end: m.t + m.duration });
      }
    }
  }
  return out;
}

/** Whether a range kind can be chosen for this sequence, and why not. */
export function rangeAvailability(seq: Sequence, kind: RangeKind): TKey | null {
  if (kind === "in_out" && (seq.in_out === null || seq.in_out === undefined)) {
    return "export.range.noInOut";
  }
  if (kind === "markers" && Object.keys(seq.markers ?? {}).length === 0) {
    return "export.range.noMarkers";
  }
  return null;
}

/** A typed custom time, in flicks; `null` when it does not read. */
export function parseRangeTime(text: string, rate: FrameRate): number | null {
  if (text.trim() === "") return null;
  const parsed = parseTimeInput(text, rate);
  return parsed.ok ? parsed.flicks : null;
}

/** What the range exports, in flicks (the first file of a marker set), or `null` when unknown. */
export function rangeSpan(
  seq: Sequence,
  draft: RangeDraft,
): { readonly start: number; readonly end: number } | null {
  const length = sequenceLength(seq);
  switch (draft.kind) {
    case "sequence":
      return length > 0 ? { start: 0, end: length } : null;
    case "in_out": {
      const io = seq.in_out;
      if (io === null || io === undefined) return null;
      const end = Math.min(io.end, length);
      return end > io.start ? { start: io.start, end } : null;
    }
    case "custom": {
      const rate = seq.format.frame_rate;
      const start = parseRangeTime(draft.start, rate);
      const end = parseRangeTime(draft.end, rate);
      if (start === null || end === null) return null;
      const stop = Math.min(end, length);
      return stop > start && start >= 0 ? { start, end: stop } : null;
    }
    case "markers": {
      const first = markerRegions(seq, draft.pairMode)[0];
      return first === undefined ? null : { start: first.start, end: first.end };
    }
  }
}

export const flicksToSeconds = (flicks: number): number => flicks / FLICKS_PER_SECOND;

/** The `range` param of `export.start`. */
export function rangeParam(draft: RangeDraft): ExportRange {
  switch (draft.kind) {
    case "sequence":
      return { kind: "sequence" };
    case "in_out":
      return { kind: "in_out" };
    case "custom":
      return { kind: "custom", in: draft.start.trim(), out: draft.end.trim() };
    case "markers":
      return { kind: "markers", pair_mode: draft.pairMode };
  }
}

// ---- the file ----------------------------------------------------------------------------------

/** "If the file exists": ask (the engine refuses and the dialog asks), keep both, replace. */
export const EXISTS_CHOICES = ["ask", "rename", "overwrite"] as const;
export type ExistsChoice = (typeof EXISTS_CHOICES)[number];

export function collisionFor(choice: ExistsChoice): Collision {
  return choice === "ask" ? "fail" : choice;
}

/** The tokens of Settings > Export > File name. */
export interface NameFacts {
  readonly project: string;
  readonly sequence: string;
  readonly preset: string;
  /** `YYYY-MM-DD`. */
  readonly date: string;
}

/** `YYYY-MM-DD` of a date, in local time. */
export function dateToken(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/**
 * A file stem from the name pattern: `{project}`, `{sequence}`, `{date}` and `{preset}` filled
 * in; other braces are kept as typed; characters a file name cannot hold become spaces.
 */
export function expandFileName(pattern: string, facts: NameFacts): string {
  const filled = (pattern.trim() === "" ? "{project}" : pattern).replace(
    /\{(project|sequence|date|preset)\}/g,
    (_, token: keyof NameFacts) => facts[token],
  );
  return safeFileStem(filled);
}

const KNOWN_EXTENSION = /\.(mp4|mov|gif|wav|mp3|m4a|flac|ogg)$/i;

/** `path` with the extension of `container` (a known extension is replaced). */
export function withExtension(path: string, container: Container): string {
  const trimmed = path.trim();
  if (trimmed === "") return trimmed;
  const ext = containerExtension(container);
  return KNOWN_EXTENSION.test(trimmed) ? trimmed.replace(KNOWN_EXTENSION, ext) : `${trimmed}${ext}`;
}

/** Whether `path` ends with the extension the preset writes. */
export function hasExtension(path: string, container: Container): boolean {
  return path.trim().toLowerCase().endsWith(containerExtension(container));
}

const MARKER_SUFFIX = "_{marker_label}_{n}";

/** The marker set's name template: `<stem>_{marker_label}_{n}.<ext>` beside the single file. */
export function markerTemplate(path: string): string {
  if (path.includes("{n}") || path.includes("{marker_label}")) return path;
  const match = /^(.*?)(\.[A-Za-z0-9]+)?$/.exec(path.trim());
  const stem = match?.[1] ?? path;
  const ext = match?.[2] ?? "";
  return `${stem}${MARKER_SUFFIX}${ext}`;
}

/** Back from a marker set: the template's suffix taken off again. */
export function singleFile(path: string): string {
  return path.replace(MARKER_SUFFIX, "");
}

/** Whether a marker-set path names each file apart (`{n}` or `{marker_label}`). */
export function isTemplate(path: string): boolean {
  return path.includes("{n}") || path.includes("{marker_label}");
}

// ---- the person's changes to the preset --------------------------------------------------------

/** Size and rate as typed in Advanced; `null` is "the preset's" (what the engine resolved). */
export interface SizeDraft {
  readonly width: string | null;
  readonly height: string | null;
  readonly fps: string | null;
}

export const PRESET_SIZE: SizeDraft = { width: null, height: null, fps: null };

const wholeNumber = (text: string): number | null => {
  const trimmed = text.trim();
  return /^\d+$/.test(trimmed) ? Number(trimmed) : null;
};

const sideOk = (text: string | null): boolean => {
  if (text === null) return true;
  const n = wholeNumber(text);
  return n !== null && n >= MIN_SIDE && n <= MAX_SIDE && n % 2 === 0;
};

/** The first typed field the engine would refuse (even sides of 16 to 8192, at most 240 fps). */
export function sizeProblem(size: SizeDraft): "width" | "height" | "fps" | null {
  if (!sideOk(size.width)) return "width";
  if (!sideOk(size.height)) return "height";
  if (size.fps !== null && parseFps(size.fps) === null) return "fps";
  return null;
}

/** The quality a preset's own rate control means; `null` when it sets a bitrate or a profile. */
export function presetQuality(spec: ExportPresetSpec): ExportQuality | null {
  const rate = spec.video?.rate_control;
  return rate?.mode === "crf" ? qualityOfCrf(rate.crf) : null;
}

/**
 * The quality the dialog starts at: the person's default (Settings > Export) for the general
 * preset, which the old dialog's quality setting was about; any other preset its own.
 */
export function startingQuality(
  spec: ExportPresetSpec,
  generalDefault: ExportQuality,
): ExportQuality | null {
  const own = presetQuality(spec);
  return own !== null && spec.id === GENERAL_PRESET ? generalDefault : own;
}

/** The changes to send with the preset (`overrides` / `{custom}`), or `null` for none. */
export function overridesFor(
  spec: ExportPresetSpec,
  size: SizeDraft,
  quality: ExportQuality | null,
): CustomExport | null {
  const out: { width?: number; height?: number; fps?: FrameRate; quality?: ExportQuality } = {};
  const width = size.width === null ? null : wholeNumber(size.width);
  const height = size.height === null ? null : wholeNumber(size.height);
  const fps = size.fps === null ? null : parseFps(size.fps);
  if (width !== null) out.width = width;
  if (height !== null) out.height = height;
  if (fps !== null) out.fps = fps;
  if (quality !== null && quality !== presetQuality(spec)) out.quality = quality;
  return Object.keys(out).length === 0 ? null : out;
}

/** The `preset` param: the id, or `{custom}` with the changes and the id they apply to. */
export function presetParam(id: string, overrides: CustomExport | null): ExportPreset {
  return overrides === null ? id : { custom: { ...overrides, preset: id } };
}

// ---- advanced ----------------------------------------------------------------------------------

/** `/advanced/video/rate_control/mode` as the form's dot path, `video.rate_control.mode`. */
export function advancedPath(pointer: string | null | undefined): string | null {
  if (pointer === null || pointer === undefined || !pointer.startsWith("/advanced/")) return null;
  return pointer
    .slice("/advanced/".length)
    .split("/")
    .map((part) => part.replace(/~1/g, "/").replace(/~0/g, "~"))
    .join(".");
}

/** What the engine's refusal of an export with missing media says (WP-2F.03c). */
export interface OfflineFacts {
  /** How many media files are missing (at least 1). */
  readonly count: number;
  /** The names the engine lists (it stops after a few; `count` is the whole number). */
  readonly names: readonly string[];
}

/**
 * Reads the engine's refusal at `/media/<id>` ("2 media files are missing: `a.mp4`, `b.wav`;
 * relink them …"). A message in another form still counts as one missing file, unnamed.
 */
export function offlineFacts(message: string): OfflineFacts {
  const said = /(\d+) media files? (?:is|are) missing: ([^;]*)/.exec(message);
  if (said === null) return { count: 1, names: [] };
  const names = [...(said[2] ?? "").matchAll(/`([^`]+)`/g)].map((m) => m[1] ?? "");
  return { count: Math.max(1, Number(said[1])), names };
}

/** The options without empty objects and unset fields; `null` when nothing is set. */
export function pruneOptions(value: unknown): Record<string, unknown> | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const out: Record<string, unknown> = {};
  for (const [key, v] of Object.entries(value)) {
    if (v === undefined || v === null || v === "") continue;
    if (typeof v === "object" && !Array.isArray(v)) {
      const inner = pruneOptions(v);
      if (inner !== null) out[key] = inner;
    } else {
      out[key] = v;
    }
  }
  return Object.keys(out).length === 0 ? null : out;
}
