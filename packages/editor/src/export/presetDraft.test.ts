// SPDX-License-Identifier: GPL-3.0-or-later
import type { ExportPresetSpec } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { PRESET_ENTRIES, PRESET_SPECS, USER_PRESET } from "../testing/exportFixtures.ts";
import { PRESET_SIZE } from "./exportModel.ts";
import {
  MAX_PRESET_ID,
  mergeAdvanced,
  nameProblem,
  type PresetDraft,
  presetFromDraft,
  presetIdFor,
  presetSlug,
} from "./presetDraft.ts";

const preset = (id: string): ExportPresetSpec =>
  PRESET_SPECS.find((p) => p.id === id) as ExportPresetSpec;

const draft = (patch: Partial<PresetDraft> = {}): PresetDraft => ({
  base: preset("h264-mp4-1080p"),
  id: "review",
  name: "Review",
  summary: "From H.264 MP4 1080p",
  size: PRESET_SIZE,
  resolved: { width: 1920, height: 1080 },
  quality: "high",
  hardware: true,
  options: {},
  ...patch,
});

describe("nameProblem", () => {
  it("wants a name of at most 80 characters", () => {
    expect(nameProblem("  ")).toBe("empty");
    expect(nameProblem("Review copy")).toBeNull();
    expect(nameProblem("x".repeat(80))).toBeNull();
    expect(nameProblem("x".repeat(81))).toBe("long");
  });
});

describe("presetSlug", () => {
  it("keeps lowercase letters, digits and single dashes", () => {
    expect(presetSlug("Review copy (720p)")).toBe("review-copy-720p");
    expect(presetSlug("  Café — final  ")).toBe("cafe-final");
  });

  it("is never empty and never longer than the engine takes", () => {
    expect(presetSlug("日本語")).toBe("my-preset");
    const long = presetSlug(`${"a".repeat(63)} b`);
    expect(long).toBe("a".repeat(63));
    expect(long.length).toBeLessThanOrEqual(MAX_PRESET_ID);
  });
});

describe("presetIdFor", () => {
  it("makes an id from a new name", () => {
    expect(presetIdFor("Review copy", PRESET_ENTRIES)).toEqual({
      id: "review-copy",
      replaces: null,
    });
  });

  it("replaces your own preset of the same name", () => {
    const found = presetIdFor(" my REVIEW copy ", [...PRESET_ENTRIES, USER_PRESET]);
    expect(found.id).toBe("my-review");
    expect(found.replaces?.name).toBe("My review copy");
  });

  it("never takes a built-in preset's id, nor another preset's of yours", () => {
    expect(presetIdFor("TikTok", PRESET_ENTRIES)).toEqual({ id: "tiktok-2", replaces: null });
    // "My review" slugs to the id of "My review copy": a different name, so a different preset.
    expect(presetIdFor("My review", [...PRESET_ENTRIES, USER_PRESET]).id).toBe("my-review-2");
  });
});

describe("mergeAdvanced", () => {
  it("is null when nothing is set", () => {
    expect(mergeAdvanced(null, {})).toBeNull();
    expect(mergeAdvanced(undefined, { video: { preset: "" } })).toBeNull();
  });

  it("puts the dialog's options over the preset's, rate control as a block", () => {
    expect(
      mergeAdvanced(
        {
          video: { rate_control: { mode: "vbr", bitrate_kbps: 8000 }, preset: "medium" },
          audio: { bitrate_kbps: 192 },
        },
        { video: { rate_control: { mode: "crf", crf: 18 } }, color: { range: "full" } },
      ),
    ).toEqual({
      video: { rate_control: { mode: "crf", crf: 18 }, preset: "medium" },
      color: { range: "full" },
      audio: { bitrate_kbps: 192 },
    });
  });
});

describe("presetFromDraft", () => {
  it("is the base preset under a new id and name when nothing was changed", () => {
    const base = preset("h264-mp4-1080p");
    expect(presetFromDraft(draft())).toEqual({
      ...base,
      id: "review",
      name: "Review",
      summary: "From H.264 MP4 1080p",
    });
  });

  it("makes a typed size and rate the preset's own, filling the side not typed", () => {
    const saved = presetFromDraft(
      draft({
        size: { width: "1280", height: null, fps: "25" },
        resolved: { width: 1920, height: 1080 },
      }),
    );
    expect(saved.size).toEqual({ kind: "fixed", width: 1280, height: 1080 });
    expect(saved.fps).toEqual({ kind: "fixed", rate: { num: 25, den: 1 } });
  });

  it("keeps the preset's size rule when a typed size does not read", () => {
    const saved = presetFromDraft(draft({ size: { width: "12x", height: null, fps: null } }));
    expect(saved.size).toEqual({ kind: "fit", width: 1920, height: 1080 });
  });

  it("turns another quality into its CRF and the hardware switch into hw_preferred", () => {
    const saved = presetFromDraft(draft({ quality: "low", hardware: false }));
    expect(saved.video?.rate_control).toEqual({ mode: "crf", crf: 28 });
    expect(saved.video?.hw_preferred).toBe(false);
    // The preset's own quality keeps its own CRF.
    expect(presetFromDraft(draft({ quality: "high" })).video?.rate_control).toEqual({
      mode: "crf",
      crf: 20,
    });
  });

  it("keeps the encoder options as the preset's advanced block", () => {
    const saved = presetFromDraft(
      draft({ options: { video: { preset: "veryslow", tune: "" }, audio: {} } }),
    );
    expect(saved.advanced).toEqual({ video: { preset: "veryslow" } });
    expect(presetFromDraft(draft()).advanced).toBeUndefined();
  });

  it("leaves a platform preset's platform out and keeps its limits", () => {
    const saved = presetFromDraft(
      draft({
        base: { ...preset("instagram-reels"), safe_area: "instagram-reels" },
        quality: null,
      }),
    );
    expect(saved.platform).toBeUndefined();
    expect(saved.safe_area).toBeUndefined();
    expect(saved.limits).toEqual({ max_duration_secs: 180, max_size_mb: 4096 });
    expect(saved.video?.rate_control).toEqual({ mode: "vbr", bitrate_kbps: 5000 });
    expect(saved.size).toEqual({ kind: "fixed", width: 1080, height: 1920 });
  });

  it("trims the name and leaves an empty summary out", () => {
    const saved = presetFromDraft(draft({ name: "  Review  ", summary: "" }));
    expect(saved.name).toBe("Review");
    expect("summary" in saved).toBe(false);
  });
});
