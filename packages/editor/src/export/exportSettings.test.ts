// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import {
  baseName,
  defaultOutputPath,
  dirName,
  encoderLabel,
  isAbsolutePath,
  parseFps,
  parseProgressLine,
  pathInFolder,
  percent,
  proposedOutputPath,
  safeFileStem,
} from "./exportSettings.ts";

describe("frame rates", () => {
  it("reads the usual ones", () => {
    expect(parseFps("30")).toEqual({ num: 30, den: 1 });
    expect(parseFps("29.97")).toEqual({ num: 30000, den: 1001 });
    expect(parseFps("23,976")).toEqual({ num: 24000, den: 1001 });
    expect(parseFps("12.5")).toEqual({ num: 12500, den: 1000 });
  });

  it("refuses what the engine would refuse", () => {
    for (const bad of ["", "0", "-5", "abc", "241", "0.0001"]) expect(parseFps(bad)).toBeNull();
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

  it("finds the folder of a path and puts a file in a folder, keeping its separator", () => {
    expect(dirName("C:\\out\\x.mp4")).toBe("C:\\out");
    expect(dirName("/home/a/x.mp4")).toBe("/home/a");
    expect(dirName("C:\\x.mp4")).toBe("C:\\");
    expect(dirName("/x.mp4")).toBe("/");
    expect(dirName("x.mp4")).toBeNull();
    expect(pathInFolder("D:\\Renders", "Film")).toBe("D:\\Renders\\Film.mp4");
    expect(pathInFolder("D:\\", "Film")).toBe("D:\\Film.mp4");
    expect(pathInFolder("/home/a", "Film")).toBe("/home/a/Film.mp4");
  });

  it("proposes the chosen or last folder, and the project's when there is none", () => {
    const folders = { last: "D:\\Last", chosen: "E:\\Chosen" };
    const doc = "C:\\p\\Film\\Film.cutweave";
    expect(proposedOutputPath("project", folders, doc, "Film")).toBe("C:\\p\\Film\\Film.mp4");
    expect(proposedOutputPath("last", folders, doc, "Film")).toBe("D:\\Last\\Film.mp4");
    expect(proposedOutputPath("chosen", folders, doc, "Film")).toBe("E:\\Chosen\\Film.mp4");
    expect(proposedOutputPath("last", { last: "", chosen: "" }, doc, "Film")).toBe(
      "C:\\p\\Film\\Film.mp4",
    );
    expect(proposedOutputPath("chosen", folders, null, "My: film")).toBe("E:\\Chosen\\My film.mp4");
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
    expect(baseName("C:\\a\\b.mp4")).toBe("b.mp4");
  });
});

describe("encoders", () => {
  it("are named for people, for every codec", () => {
    expect(encoderLabel("h264_nvenc")).toBe("NVIDIA NVENC");
    expect(encoderLabel("hevc_amf")).toBe("AMD AMF");
    expect(encoderLabel("av1_qsv")).toBe("Intel Quick Sync");
    expect(encoderLabel("hevc_videotoolbox")).toBe("Apple VideoToolbox");
    expect(encoderLabel("h264_mystery")).toBe("h264_mystery");
    expect(encoderLabel("libx264")).toBe("libx264");
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
