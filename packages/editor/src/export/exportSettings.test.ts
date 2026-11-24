// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import {
  baseName,
  defaultDraft,
  defaultOutputPath,
  encoderLabel,
  formatProblem,
  fpsText,
  hardwareEncoders,
  hasMp4Extension,
  hwFor,
  isAbsolutePath,
  parseFps,
  parseProgressLine,
  percent,
  presetFor,
  presetSize,
  safeFileStem,
} from "./exportSettings.ts";

const hd = { width: 1920, height: 1080, fps: { num: 30, den: 1 } };

describe("the preset's size", () => {
  it("keeps 1080p and fits larger and portrait canvases without enlarging", () => {
    expect(presetSize(hd)).toEqual({ width: 1920, height: 1080 });
    expect(presetSize({ width: 3840, height: 2160 })).toEqual({ width: 1920, height: 1080 });
    expect(presetSize({ width: 1080, height: 1920 })).toEqual({ width: 1080, height: 1920 });
    expect(presetSize({ width: 2160, height: 3840 })).toEqual({ width: 1080, height: 1920 });
    expect(presetSize({ width: 1280, height: 720 })).toEqual({ width: 1280, height: 720 });
    expect(presetSize({ width: 1001, height: 563 })).toEqual({ width: 1000, height: 562 });
  });
});

describe("frame rates", () => {
  it("reads and writes the usual ones", () => {
    expect(parseFps("30")).toEqual({ num: 30, den: 1 });
    expect(parseFps("29.97")).toEqual({ num: 30000, den: 1001 });
    expect(parseFps("23,976")).toEqual({ num: 24000, den: 1001 });
    expect(parseFps("12.5")).toEqual({ num: 12500, den: 1000 });
    expect(fpsText({ num: 30000, den: 1001 })).toBe("29.97");
    expect(fpsText({ num: 25, den: 1 })).toBe("25");
  });

  it("refuses what the engine would refuse", () => {
    for (const bad of ["", "0", "-5", "abc", "241", "0.0001"]) expect(parseFps(bad)).toBeNull();
  });
});

describe("the format fields", () => {
  it("start at the preset and are valid", () => {
    const draft = defaultDraft(hd);
    expect(draft).toEqual({ width: "1920", height: "1080", fps: "30", quality: "high" });
    expect(formatProblem(draft)).toBeNull();
    expect(presetFor(draft, hd)).toBe("h264-mp4-1080p");
  });

  it("name the first bad field", () => {
    const draft = defaultDraft(hd);
    expect(formatProblem({ ...draft, width: "1921" })).toBe("width");
    expect(formatProblem({ ...draft, width: "8" })).toBe("width");
    expect(formatProblem({ ...draft, height: "9000" })).toBe("height");
    expect(formatProblem({ ...draft, height: "" })).toBe("height");
    expect(formatProblem({ ...draft, fps: "300" })).toBe("fps");
  });

  it("make a custom export once one value differs from the preset", () => {
    const draft = defaultDraft(hd);
    expect(presetFor({ ...draft, quality: "low" }, hd)).toEqual({
      custom: { width: 1920, height: 1080, fps: { num: 30, den: 1 }, quality: "low" },
    });
    expect(presetFor({ ...draft, width: "1280", height: "720" }, hd)).toEqual({
      custom: { width: 1280, height: 720, fps: { num: 30, den: 1 }, quality: "high" },
    });
    expect(presetFor({ ...draft, fps: "24" }, hd)).toEqual({
      custom: { width: 1920, height: 1080, fps: { num: 24, den: 1 }, quality: "high" },
    });
  });

  it("see an NTSC rate typed as 29.97 as the sequence's own rate", () => {
    const ntsc = { ...hd, fps: { num: 30000, den: 1001 } };
    expect(presetFor(defaultDraft(ntsc), ntsc)).toBe("h264-mp4-1080p");
  });
});

describe("hardware choice", () => {
  it("is auto or off", () => {
    expect(hwFor(true)).toBe("auto");
    expect(hwFor(false)).toBe("off");
  });
});

describe("the output path", () => {
  it("defaults to the project folder, named after the project", () => {
    expect(defaultOutputPath("C:\\Work\\Film\\Film.cutweave", "Film")).toBe(
      "C:\\Work\\Film\\Film.mp4",
    );
    expect(defaultOutputPath("/home/a/Film/Film.cutweave", "Film")).toBe("/home/a/Film/Film.mp4");
    expect(defaultOutputPath(null, "Untitled project")).toBe("Untitled project.mp4");
  });

  it("makes a file name of any project name", () => {
    expect(safeFileStem('a:b/c*"d"?')).toBe("a b c d");
    expect(safeFileStem("  ")).toBe("Export");
    expect(safeFileStem("Film. ")).toBe("Film");
  });

  it("checks what export.start takes", () => {
    expect(isAbsolutePath("C:\\a\\b.mp4")).toBe(true);
    expect(isAbsolutePath("/a/b.mp4")).toBe(true);
    expect(isAbsolutePath("b.mp4")).toBe(false);
    expect(hasMp4Extension("x.MP4")).toBe(true);
    expect(hasMp4Extension("x.mov")).toBe(false);
    expect(baseName("C:\\a\\b.mp4")).toBe("b.mp4");
  });
});

describe("encoders", () => {
  it("are named for people", () => {
    expect(encoderLabel("h264_nvenc")).toBe("NVIDIA NVENC");
    expect(encoderLabel("h264_amf")).toBe("AMD AMF");
    expect(encoderLabel("h264_qsv")).toBe("Intel Quick Sync");
    expect(encoderLabel("h264_mystery")).toBe("h264_mystery");
  });

  it("are read from media.capabilities, in order", () => {
    const answer = {
      hw_decode: [],
      hw_encode: [
        { device: "cuda", codec: "h264", implementation: "h264_nvenc" },
        { device: "d3d11", codec: "h264", implementation: "h264_amf" },
        { device: "x", codec: "h264", implementation: "h264_nvenc" },
      ],
      codecs: [],
    };
    expect(hardwareEncoders(answer)).toEqual(["h264_nvenc", "h264_amf"]);
    expect(hardwareEncoders({ hw_encode: [] })).toEqual([]);
    expect(hardwareEncoders({})).toBeNull();
    expect(
      hardwareEncoders({ available: false, hw_encode: [], hw_decode: [], codecs: [] }),
    ).toBeNull();
    expect(hardwareEncoders(null)).toBeNull();
  });
});

describe("the progress line", () => {
  it("gives speed and time left", () => {
    expect(parseProgressLine("frame 120 of 600, 31.2 fps, about 0:15 left")).toEqual({
      fps: "31.2",
      eta: "0:15",
      restartedInSoftware: false,
    });
  });

  it("notices the restart in software", () => {
    const line = parseProgressLine(
      "frame 3 of 600, 20 fps, about 0:30 left (again in software: h264_nvenc failed)",
    );
    expect(line.restartedInSoftware).toBe(true);
  });

  it("copes with no line at all", () => {
    expect(parseProgressLine(null)).toEqual({ fps: null, eta: null, restartedInSoftware: false });
  });

  it("turns the fraction into a percent", () => {
    expect(percent(0.426)).toBe(43);
    expect(percent(null)).toBe(0);
    expect(percent(2)).toBe(100);
  });
});
