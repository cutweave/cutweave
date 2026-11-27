// SPDX-License-Identifier: GPL-3.0-or-later
// The format new projects start with (#221): how the app preference `editing.sequence_format`
// spells a sequence format, and what `project.new` gets from it.
import { validatePreference } from "@cutweave/preferences";
import type { SequenceFormat } from "@cutweave/protocol";
import { describe, expect, it } from "vitest";
import { formatPreference, parseFormatPreference } from "./newSequenceFormat.ts";

const format = (more: Partial<SequenceFormat> = {}): SequenceFormat =>
  ({
    width: 1080,
    height: 1920,
    frame_rate: { num: 30000, den: 1001 },
    sample_rate: 44100,
    color_space: "rec709",
    background: "#1A2B3C",
    ...more,
  }) as SequenceFormat;

describe("the format of new projects as a preference", () => {
  it("spells a format on one line the preferences schema accepts", () => {
    const text = formatPreference(format());
    expect(text).toBe("1080x1920 30000/1001 44100 rec709 #1a2b3c");
    expect(validatePreference("editing.sequence_format", text)).toEqual({ ok: true, value: text });
  });

  it("reads the line back as the settings project.new takes", () => {
    expect(parseFormatPreference("1080x1920 30000/1001 44100 rec709 #1a2b3c")).toEqual({
      width: 1080,
      height: 1920,
      frame_rate: { num: 30000, den: 1001 },
      sample_rate: 44100,
      color_space: "rec709",
      background: "#1a2b3c",
    });
    // A background with alpha keeps it.
    expect(parseFormatPreference(formatPreference(format({ background: "#00000080" })))).toEqual(
      expect.objectContaining({ background: "#00000080" }),
    );
  });

  it("is nothing when the preference is not set", () => {
    expect(parseFormatPreference("")).toBeNull();
    expect(validatePreference("editing.sequence_format", "")).toEqual({ ok: true, value: "" });
  });

  it("is nothing for a line the engine would refuse, so a new project can still be created", () => {
    for (const text of [
      "1080 by 1920",
      "0x1080 30/1 48000 rec709 #000000",
      "20000x1080 30/1 48000 rec709 #000000",
      "1920x1080 2000/1 48000 rec709 #000000",
      // 11 fps is not a whole number of flicks per frame.
      "1920x1080 11/1 48000 rec709 #000000",
      "1920x1080 30/1 4000 rec709 #000000",
      "1920x1080 30/1 48000 rec709 black",
    ]) {
      expect(parseFormatPreference(text), text).toBeNull();
    }
  });

  it("does not spell a format it could not read back", () => {
    expect(formatPreference(format({ width: 0 }))).toBe("");
    expect(formatPreference(format({ background: "black" }))).toBe("");
  });

  it("refuses a hand-written value that is not a format", () => {
    expect(validatePreference("editing.sequence_format", "1080 by 1920").ok).toBe(false);
    expect(validatePreference("editing.sequence_format", 1080).ok).toBe(false);
  });
});
