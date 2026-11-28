// SPDX-License-Identifier: GPL-3.0-or-later
// Export presets and encoders for the export dialog's tests (WP-2G.04): a hand-written subset of
// the built-in pack (`plugins/presets-core/presets.json`) and an `export.list_encoders` answer,
// with only the fields the editor reads.

import type {
  CodecEncoders,
  ExportPresetEntry,
  ExportPresetSpec,
  Project,
  ResolvePresetOutput,
} from "@cutweave/protocol";

const FLICKS = 705_600_000;

const platform = (id: string, name: string) => ({
  id,
  name,
  spec_url: "https://example.invalid/spec",
  checked: "2026-11-24",
});

type VideoSpec = NonNullable<ExportPresetSpec["video"]>;

const h264 = (rate: VideoSpec["rate_control"]): VideoSpec => ({
  codec: "h264",
  rate_control: rate,
  hw_preferred: true,
});

const aac = { codec: "aac", bitrate_kbps: 192, sample_rate: 48000, channels: 2 } as const;

function spec(fields: Partial<ExportPresetSpec> & Pick<ExportPresetSpec, "id" | "name">) {
  return {
    schema_version: 1,
    container: "mp4",
    audio: aac,
    size: { kind: "follow_sequence" },
    fps: { kind: "follow_sequence" },
    limits: {},
    ...fields,
  } as ExportPresetSpec;
}

const platformPreset = (
  id: string,
  name: string,
  plat: [string, string],
  width: number,
  height: number,
  maxFps: number,
  kbps: number,
  limits: { max_duration_secs: number; max_size_mb: number },
  summary: string,
) =>
  spec({
    id,
    name,
    summary,
    video: h264({ mode: "vbr", bitrate_kbps: kbps }),
    size: { kind: "fixed", width, height },
    fps: { kind: "follow_sequence", max: { num: maxFps, den: 1 } },
    limits,
    platform: platform(plat[0], plat[1]),
  });

export const PRESET_SPECS: readonly ExportPresetSpec[] = [
  platformPreset(
    "youtube-1080p",
    "YouTube 1080p",
    ["youtube", "YouTube"],
    1920,
    1080,
    60,
    8000,
    { max_duration_secs: 43200, max_size_mb: 262144 },
    "1920 x 1080, up to 60 fps, 8 Mbps",
  ),
  platformPreset(
    "youtube-4k",
    "YouTube 4K",
    ["youtube", "YouTube"],
    3840,
    2160,
    60,
    40000,
    { max_duration_secs: 43200, max_size_mb: 262144 },
    "3840 x 2160, up to 60 fps, 40 Mbps",
  ),
  platformPreset(
    "youtube-shorts",
    "YouTube Shorts",
    ["youtube-shorts", "YouTube Shorts"],
    1080,
    1920,
    60,
    8000,
    { max_duration_secs: 180, max_size_mb: 262144 },
    "1080 x 1920, up to 3 minutes",
  ),
  platformPreset(
    "instagram-reels",
    "Instagram Reels",
    ["instagram-reels", "Instagram Reels"],
    1080,
    1920,
    30,
    5000,
    { max_duration_secs: 180, max_size_mb: 4096 },
    "1080 x 1920, up to 30 fps",
  ),
  platformPreset(
    "instagram-feed-4x5",
    "Instagram feed 4:5",
    ["instagram-feed", "Instagram feed"],
    1080,
    1350,
    30,
    5000,
    { max_duration_secs: 3600, max_size_mb: 4096 },
    "1080 x 1350, up to 30 fps",
  ),
  platformPreset(
    "tiktok",
    "TikTok",
    ["tiktok", "TikTok"],
    1080,
    1920,
    30,
    6000,
    { max_duration_secs: 600, max_size_mb: 4096 },
    "1080 x 1920, up to 30 fps",
  ),
  platformPreset(
    "x-landscape",
    "X landscape",
    ["x", "X"],
    1920,
    1080,
    60,
    8000,
    { max_duration_secs: 140, max_size_mb: 512 },
    "1920 x 1080, up to 60 fps, 2:20",
  ),
  platformPreset(
    "x-portrait",
    "X portrait",
    ["x", "X"],
    1080,
    1920,
    60,
    8000,
    { max_duration_secs: 140, max_size_mb: 512 },
    "1080 x 1920, up to 60 fps, 2:20",
  ),
  spec({
    id: "h264-mp4-1080p",
    name: "H.264 MP4 1080p",
    summary: "Fit into 1920 x 1080, sequence frame rate, high quality",
    video: h264({ mode: "crf", crf: 20 }),
    size: { kind: "fit", width: 1920, height: 1080 },
  }),
  spec({
    id: "h264-draft",
    name: "H.264 draft",
    summary: "Fit into 1280 x 720, small and fast",
    video: h264({ mode: "crf", crf: 28 }),
    size: { kind: "fit", width: 1280, height: 720 },
  }),
  spec({
    id: "archive-hevc",
    name: "Archive (HEVC)",
    summary: "Sequence size, high quality HEVC 10-bit in MP4",
    video: {
      codec: "hevc",
      rate_control: { mode: "crf", crf: 16 },
      hw_preferred: false,
    } as VideoSpec,
  }),
  spec({
    id: "prores-422-hq",
    name: "ProRes 422 HQ",
    summary: "Sequence size and rate, ProRes 422 HQ in MOV",
    container: "mov",
    audio: { codec: "pcm", sample_rate: 48000, channels: 2 },
    video: {
      codec: "prores",
      rate_control: { mode: "profile" },
      profile: "422hq",
      hw_preferred: false,
    } as VideoSpec,
  }),
  spec({
    id: "gif",
    name: "GIF",
    summary: "Up to 480 px, 15 fps, palette, loops forever",
    container: "gif",
    audio: null,
    video: {
      codec: "gif",
      rate_control: { mode: "profile" },
      hw_preferred: false,
    } as VideoSpec,
    size: { kind: "fit", width: 480, height: 480 },
    fps: { kind: "fixed", rate: { num: 15, den: 1 } },
  }),
];

/** The `export.list_presets` answer: GIF is listed but not supported by this build. */
export const PRESET_ENTRIES: readonly ExportPresetEntry[] = PRESET_SPECS.map((preset) => ({
  source: "builtin",
  supported: preset.id !== "gif",
  unsupported_reason: preset.id === "gif" ? "GIF export arrives with WP-2G.02b" : null,
  preset,
}));

/** A preset saved by the person. */
export const USER_PRESET: ExportPresetEntry = {
  source: "user",
  supported: true,
  preset: spec({
    id: "my-review",
    name: "My review copy",
    video: h264({ mode: "crf", crf: 26 }),
  }),
};

/** The `export.list_encoders` answer of a machine with NVENC. */
export const ENCODERS: readonly CodecEncoders[] = [
  {
    codec: "h264",
    available: true,
    software: "libx264",
    hardware: [{ implementation: "h264_nvenc", device: "cuda" }],
    containers: ["mp4", "mov"],
    bit_depths: [8],
    profiles: ["baseline", "main", "high"],
  },
  {
    codec: "hevc",
    available: true,
    software: "libx265",
    hardware: [],
    containers: ["mp4", "mov"],
    bit_depths: [8, 10],
    profiles: ["main", "main10"],
  },
];

/** A schema like `export.describe_options{codec: "h264"}` answers (a subset). */
export const H264_OPTIONS = {
  type: "object",
  properties: {
    video: {
      type: "object",
      properties: {
        rate_control: {
          type: "object",
          required: ["mode"],
          properties: {
            mode: { enum: ["crf", "cbr", "vbr", "cq"] },
            crf: { type: "integer", minimum: 0, maximum: 51 },
            bitrate_kbps: { type: "integer", minimum: 100, maximum: 200000 },
          },
        },
        preset: { enum: ["ultrafast", "medium", "veryslow"] },
      },
    },
    audio: {
      type: "object",
      properties: { channels: { enum: ["mono", "stereo"] } },
    },
  },
};

/** A project with one sequence of `seconds`, `width` x `height` at 30 fps, and markers. */
export function exportProject(
  width: number,
  height: number,
  seconds: number,
  extra: { markers?: object; in_out?: object | null; background_fill?: object | null } = {},
): Project {
  return {
    meta: { name: "Film" },
    settings: {},
    media: {},
    sequences: {
      seq_1: {
        id: "seq_1",
        name: "Main",
        format: {
          width,
          height,
          frame_rate: { num: 30, den: 1 },
          ...(extra.background_fill != null && { background_fill: extra.background_fill }),
        },
        tracks: {
          trk_1: {
            id: "trk_1",
            items: { itm_1: { id: "itm_1", start: 0, duration: Math.round(seconds * FLICKS) } },
          },
        },
        track_order: ["trk_1"],
        markers: extra.markers ?? {},
        in_out: extra.in_out ?? null,
        custom: {},
      },
    },
    custom: {},
  } as unknown as Project;
}

/**
 * A preset of the user's that keeps alpha (ProRes 4444 with the alpha pixel format): the only
 * kind of preset whose file shows a transparent background (issue #238).
 */
export const ALPHA_PRESET: ExportPresetEntry = {
  source: "user",
  supported: true,
  preset: spec({
    id: "my-alpha",
    name: "My alpha master",
    container: "mov",
    video: {
      codec: "prores",
      rate_control: { mode: "profile" },
      profile: "4444",
      pix_fmt: "yuva444p10le",
      hw_preferred: false,
    },
    audio: { codec: "pcm", sample_rate: 48000, channels: 2 },
  }),
};

/**
 * A plausible `export.resolve_preset` answer, as the engine computes it for these presets.
 * `transparent`: the sequence's background is transparent, which the engine warns about for
 * every file that keeps no alpha channel.
 */
export function resolveFor(
  presetId: string,
  seq: { width: number; height: number; seconds: number; transparent?: boolean },
): ResolvePresetOutput {
  const preset =
    [...PRESET_SPECS, ALPHA_PRESET.preset].find((p) => p.id === presetId) ?? PRESET_SPECS[0];
  const p = preset as ExportPresetSpec;
  const warnings: ResolvePresetOutput["warnings"] = [];
  let width = seq.width;
  let height = seq.height;
  if (p.size.kind === "fixed") {
    const same =
      Math.abs(seq.width * p.size.height - seq.height * p.size.width) * 100 <=
      Math.max(seq.width * p.size.height, seq.height * p.size.width);
    if (same) {
      width = p.size.width;
      height = p.size.height;
    } else {
      const scale = Math.min(p.size.width / seq.width, p.size.height / seq.height);
      width = Math.floor((seq.width * scale) / 2) * 2;
      height = Math.floor((seq.height * scale) / 2) * 2;
      warnings.push({ code: "AspectMismatch", message: "aspect" });
    }
  }
  const max = p.limits?.max_duration_secs;
  if (max !== undefined && max !== null && seq.seconds > max) {
    warnings.push({ code: "DurationOverLimit", message: "too long" });
  }
  const alpha = p.video?.codec === "prores" && (p.video.pix_fmt ?? "").startsWith("yuva");
  if (seq.transparent === true && !alpha) {
    warnings.push({ code: "TransparentBackground", message: "transparent background" });
  }
  const rate = p.video?.rate_control;
  const kbps = rate?.mode === "vbr" || rate?.mode === "cbr" ? rate.bitrate_kbps : null;
  return {
    preset: p.id,
    sequence: "seq_1" as ResolvePresetOutput["sequence"],
    width,
    height,
    frame_rate: { num: 30, den: 1 },
    estimated_kib: kbps === null ? null : Math.ceil(((kbps + 192) * 125 * seq.seconds) / 1024),
    supported: p.id !== "gif",
    unsupported_reason: null,
    warnings,
  };
}
