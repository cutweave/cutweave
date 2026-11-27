// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2G.04: what the export dialog works out on its own (picker groups, summary, estimate,
// warnings, ranges, file names, the request).
import type { ExportPresetSpec, Sequence } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import {
  exportProject,
  PRESET_ENTRIES,
  PRESET_SPECS,
  USER_PRESET,
} from "../testing/exportFixtures.ts";
import {
  advancedPath,
  clockText,
  codecsText,
  collisionFor,
  estimateSize,
  expandFileName,
  fitsSequence,
  groupPresets,
  hasExtension,
  humanBytes,
  isTemplate,
  markerRegions,
  markerTemplate,
  offlineFacts,
  overridesFor,
  PRESET_SIZE,
  presetParam,
  presetQuality,
  pruneOptions,
  rangeAvailability,
  rangeParam,
  rangeSpan,
  ratioText,
  singleFile,
  sizeProblem,
  startingPreset,
  startingQuality,
  warningText,
  withExtension,
} from "./exportModel.ts";

const F = 705_600_000;
const spec = (id: string) => PRESET_SPECS.find((p) => p.id === id) as ExportPresetSpec;
const seqOf = (project: ReturnType<typeof exportProject>) =>
  Object.values(project.sequences)[0] as Sequence;

describe("offlineFacts", () => {
  it("reads the count and the names of the engine's refusal", () => {
    expect(
      offlineFacts(
        "1 media file is missing: `beach day.mp4`; relink them (media.relink_folder, " +
          "media.relink) or pass `allow_offline: true` to export a slate",
      ),
    ).toEqual({ count: 1, names: ["beach day.mp4"] });
    expect(
      offlineFacts(
        "invalid value at `/media/med_1`: 11 media files are missing: `0.mp4`, `1.mp4` and 9 " +
          "more; relink them or pass `allow_offline: true`",
      ),
    ).toEqual({ count: 11, names: ["0.mp4", "1.mp4"] });
  });

  it("counts one unnamed file when the message has another form", () => {
    expect(offlineFacts("media is offline")).toEqual({ count: 1, names: [] });
  });
});

describe("the preset picker", () => {
  it("puts the platform presets made for a 9:16 sequence first, then each platform, then General", () => {
    const groups = groupPresets([...PRESET_ENTRIES, USER_PRESET], { width: 1080, height: 1920 });
    expect(groups.map((g) => g.id)).toEqual([
      "match",
      "brand:youtube",
      "brand:instagram",
      "brand:x",
      "general",
      "user",
    ]);
    expect(groups[0]?.label).toEqual({ key: "export.presets.match", vars: { ratio: "9:16" } });
    expect(groups[0]?.options.map((o) => o.id)).toEqual([
      "youtube-shorts",
      "instagram-reels",
      "tiktok",
      "x-portrait",
    ]);
    expect(groups[1]).toMatchObject({ label: { name: "YouTube" } });
    expect(groups[1]?.options.map((o) => o.id)).toEqual(["youtube-1080p", "youtube-4k"]);
    expect(groups[2]).toMatchObject({ label: { name: "Instagram" } });
    expect(groups[3]?.options.map((o) => o.id)).toEqual(["x-landscape"]);
    expect(groups[4]?.options.find((o) => o.id === "gif")).toMatchObject({
      supported: false,
      reason: "GIF export arrives with WP-2G.02b",
    });
    expect(groups[5]?.options.map((o) => o.id)).toEqual(["my-review"]);
  });

  it("puts the landscape ones first on a 16:9 sequence; TikTok then stands under its own name", () => {
    const groups = groupPresets(PRESET_ENTRIES, { width: 3840, height: 2160 });
    expect(groups[0]?.options.map((o) => o.id)).toEqual([
      "youtube-1080p",
      "youtube-4k",
      "x-landscape",
    ]);
    expect(groups.find((g) => g.id === "brand:tiktok")?.label).toEqual({ name: "TikTok" });
    expect(fitsSequence(spec("tiktok"), { width: 1080, height: 1920 })).toBe(true);
    expect(fitsSequence(spec("h264-mp4-1080p"), { width: 1080, height: 1920 })).toBe(false);
  });

  it("starts at the default preset, the last one used, else the general one, never an unsupported one", () => {
    expect(startingPreset(PRESET_ENTRIES, ["tiktok"])).toBe("tiktok");
    expect(startingPreset(PRESET_ENTRIES, [""])).toBe("h264-mp4-1080p");
    expect(startingPreset(PRESET_ENTRIES, ["gif"])).toBe("h264-mp4-1080p");
    expect(startingPreset(PRESET_ENTRIES, ["gone"])).toBe("h264-mp4-1080p");
  });
});

describe("the summary", () => {
  it("names the codecs, the ratio, the clock and the size", () => {
    expect(codecsText(spec("instagram-reels"))).toBe("H.264 + AAC");
    expect(codecsText(spec("prores-422-hq"))).toBe("ProRes + PCM");
    expect(codecsText(spec("gif"))).toBe("GIF");
    expect(ratioText(1080, 1920)).toBe("9:16");
    expect(ratioText(1080, 1350)).toBe("4:5");
    expect(clockText(200)).toBe("3:20");
    expect(clockText(3725)).toBe("1:02:05");
    expect(humanBytes(54_300_000)).toBe("55 MB");
    expect(humanBytes(3_450_000_000)).toBe("3.4 GB");
  });

  it("takes the engine's estimate for a bitrate preset and guesses an upper end for a quality", () => {
    const fps = { num: 30, den: 1 };
    const base = { width: 1080, height: 1920, fps, seconds: 10 };
    expect(
      estimateSize({ ...base, spec: spec("instagram-reels"), quality: null, resolvedKib: 6339 }),
    ).toEqual({ bytes: 6339 * 1024, upTo: false });
    // 1080p30 at CRF 20: 150 thousandths of a bit per pixel, plus 192 kb/s audio, plus 5 %.
    const general = estimateSize({
      ...base,
      width: 1920,
      height: 1080,
      spec: spec("h264-mp4-1080p"),
      quality: null,
      resolvedKib: null,
    });
    expect(general?.upTo).toBe(true);
    expect(general?.bytes).toBe(Math.ceil((((1920 * 1080 * 30 * 0.15 + 192_000) * 10) / 8) * 1.05));
    // A quality chosen for a bitrate preset replaces its bitrate.
    expect(
      estimateSize({ ...base, spec: spec("instagram-reels"), quality: "low", resolvedKib: 6339 })
        ?.upTo,
    ).toBe(true);
    expect(
      estimateSize({ ...base, spec: spec("prores-422-hq"), quality: null, resolvedKib: null }),
    ).toBeNull();
    expect(
      estimateSize({ ...base, seconds: 0, spec: spec("tiktok"), quality: null, resolvedKib: 1 }),
    ).toBeNull();
  });
});

describe("warnings in words", () => {
  const ctx = {
    seq: { width: 1080, height: 1920 },
    width: 608,
    height: 1080,
    fps: { num: 30, den: 1 },
    seconds: 200,
    estimate: { bytes: 600 * 1024 * 1024, upTo: false },
  };

  it("says a preset of another shape fits with bars, at what size", () => {
    expect(
      warningText(
        { code: "AspectMismatch", message: "x" },
        { ...ctx, spec: spec("youtube-1080p") },
      ),
    ).toMatchObject({
      key: "export.warnings.aspect",
      vars: {
        preset: "YouTube 1080p",
        presetRatio: "16:9",
        seqRatio: "9:16",
        width: "608",
        height: "1080",
      },
    });
  });

  it("gives the platform's limits as clock times and sizes", () => {
    const reels = { ...ctx, spec: spec("instagram-reels") };
    expect(warningText({ code: "DurationOverLimit", message: "x" }, reels)).toMatchObject({
      key: "export.warnings.duration",
      vars: { platform: "Instagram Reels", limit: "3:00", length: "3:20" },
    });
    expect(
      warningText({ code: "SizeOverLimit", message: "x" }, { ...ctx, spec: spec("x-portrait") }),
    ).toMatchObject({ vars: { platform: "X", size: "630 MB", limit: "537 MB" } });
    expect(warningText({ code: "FrameRateCapped", message: "x" }, reels)).toMatchObject({
      vars: { max: "30", fps: "30" },
    });
    expect(warningText({ code: "Mystery" as never, message: "engine says" }, reels)).toMatchObject({
      key: null,
      message: "engine says",
    });
  });
});

describe("ranges", () => {
  const markers = {
    m1: { id: "m1", t: 2 * F, duration: 3 * F, label: "Intro", color: "#fff", kind: "user" },
    m2: { id: "m2", t: 10 * F, duration: null, label: "", color: "#fff", kind: "user" },
    m3: { id: "m3", t: 6 * F, duration: 1 * F, label: "Hook", color: "#fff", kind: "user" },
  };
  const seq = seqOf(
    exportProject(1080, 1920, 20, { markers, in_out: { start: 4 * F, end: 30 * F } }),
  );
  const bare = seqOf(exportProject(1080, 1920, 20));

  it("makes one file per marker: by duration (markers without one skipped) or marker to marker", () => {
    expect(markerRegions(seq, "duration").map((r) => [r.label, r.start / F, r.end / F])).toEqual([
      ["Intro", 2, 5],
      ["Hook", 6, 7],
    ]);
    expect(markerRegions(seq, "pairs").map((r) => [r.start / F, r.end / F])).toEqual([
      [2, 6],
      [6, 10],
    ]);
  });

  it("offers in to out and markers only when the sequence has them", () => {
    expect(rangeAvailability(bare, "in_out")).toBe("export.range.noInOut");
    expect(rangeAvailability(bare, "markers")).toBe("export.range.noMarkers");
    expect(rangeAvailability(seq, "in_out")).toBeNull();
    expect(rangeAvailability(seq, "markers")).toBeNull();
  });

  it("knows the span: an out point past the end stops at the end; typed times read like the engine", () => {
    const at = (kind: "sequence" | "in_out" | "custom" | "markers", start = "", end = "") =>
      rangeSpan(seq, { kind, start, end, pairMode: "duration" });
    expect(at("sequence")).toEqual({ start: 0, end: 20 * F });
    expect(at("in_out")).toEqual({ start: 4 * F, end: 20 * F });
    expect(at("custom", "00:00:01:00", "5s")).toEqual({ start: F, end: 5 * F });
    expect(at("custom", "5s", "1s")).toBeNull();
    expect(at("custom", "nope", "1s")).toBeNull();
    expect(at("markers")).toEqual({ start: 2 * F, end: 5 * F });
  });

  it("becomes the range param of export.start", () => {
    const draft = { kind: "custom", start: " 1s ", end: "4s", pairMode: "duration" } as const;
    expect(rangeParam(draft)).toEqual({ kind: "custom", in: "1s", out: "4s" });
    expect(rangeParam({ ...draft, kind: "markers", pairMode: "pairs" })).toEqual({
      kind: "markers",
      pair_mode: "pairs",
    });
    expect(rangeParam({ ...draft, kind: "sequence" })).toEqual({ kind: "sequence" });
  });
});

describe("the file", () => {
  it("fills the name pattern and keeps only what a file name can hold", () => {
    const facts = { project: "Film", sequence: "Main: cut", preset: "TikTok", date: "2026-11-26" };
    expect(expandFileName("{project}", facts)).toBe("Film");
    expect(expandFileName("{project}_{sequence}_{preset}_{date}", facts)).toBe(
      "Film_Main  cut_TikTok_2026-11-26".replace("  ", " "),
    );
    expect(expandFileName("", facts)).toBe("Film");
    expect(expandFileName("{unknown}", facts)).toBe("{unknown}");
  });

  it("gives the path the preset's extension", () => {
    expect(withExtension("C:\\a\\Film.mp4", "mov")).toBe("C:\\a\\Film.mov");
    expect(withExtension("/a/Film", "mp4")).toBe("/a/Film.mp4");
    expect(hasExtension("/a/Film.MOV", "mov")).toBe(true);
    expect(hasExtension("/a/Film.mp4", "mov")).toBe(false);
  });

  it("names the files of a marker set and comes back", () => {
    expect(markerTemplate("/a/Film.mp4")).toBe("/a/Film_{marker_label}_{n}.mp4");
    expect(markerTemplate("/a/{seq}_{n}.mp4")).toBe("/a/{seq}_{n}.mp4");
    expect(isTemplate("/a/Film_{marker_label}_{n}.mp4")).toBe(true);
    expect(singleFile("/a/Film_{marker_label}_{n}.mp4")).toBe("/a/Film.mp4");
  });

  it("asks before replacing unless told to keep both or replace", () => {
    expect(collisionFor("ask")).toBe("fail");
    expect(collisionFor("rename")).toBe("rename");
    expect(collisionFor("overwrite")).toBe("overwrite");
  });
});

describe("the person's changes", () => {
  it("starts the general preset at the default quality and others at their own", () => {
    expect(startingQuality(spec("h264-mp4-1080p"), "medium")).toBe("medium");
    expect(startingQuality(spec("h264-draft"), "high")).toBe("low");
    expect(startingQuality(spec("archive-hevc"), "medium")).toBe("high");
    expect(startingQuality(spec("tiktok"), "medium")).toBeNull();
    expect(presetQuality(spec("h264-mp4-1080p"))).toBe("high");
  });

  it("sends the preset alone until something differs, then {custom} on top of it", () => {
    const general = spec("h264-mp4-1080p");
    expect(overridesFor(general, PRESET_SIZE, "high")).toBeNull();
    expect(presetParam("h264-mp4-1080p", null)).toBe("h264-mp4-1080p");
    const changed = overridesFor(general, { width: "3840", height: "2160", fps: "29.97" }, "low");
    expect(changed).toEqual({
      width: 3840,
      height: 2160,
      fps: { num: 30000, den: 1001 },
      quality: "low",
    });
    expect(presetParam("h264-mp4-1080p", changed)).toEqual({
      custom: { ...changed, preset: "h264-mp4-1080p" },
    });
  });

  it("names the first typed field the engine would refuse", () => {
    expect(sizeProblem(PRESET_SIZE)).toBeNull();
    expect(sizeProblem({ ...PRESET_SIZE, width: "1921" })).toBe("width");
    expect(sizeProblem({ ...PRESET_SIZE, height: "8" })).toBe("height");
    expect(sizeProblem({ ...PRESET_SIZE, fps: "300" })).toBe("fps");
  });
});

describe("encoder options", () => {
  it("turns the engine's pointer into the form's path", () => {
    expect(advancedPath("/advanced/video/rate_control/mode")).toBe("video.rate_control.mode");
    expect(advancedPath("/path")).toBeNull();
    expect(advancedPath(null)).toBeNull();
  });

  it("sends only what was set", () => {
    expect(pruneOptions({})).toBeNull();
    expect(pruneOptions({ video: { rate_control: {} }, audio: { channels: "mono" } })).toEqual({
      audio: { channels: "mono" },
    });
    expect(pruneOptions({ video: { gop: { keyint: 60, closed_gop: false } } })).toEqual({
      video: { gop: { keyint: 60, closed_gop: false } },
    });
  });
});
