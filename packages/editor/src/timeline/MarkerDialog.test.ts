// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Issue #222: what the marker's Edit… dialog sends, and how a stored colour names its label.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { LABEL_COLORS, labelColorOf, labelColorValue } from "./labelColors.ts";
import { lengthArg, lengthText, markerEdit } from "./MarkerDialog.tsx";

const F = 705_600_000;

describe("the marker dialog's length field", () => {
  it("shows a range marker's length in seconds and a point marker as empty", () => {
    expect(lengthText(null)).toBe("");
    expect(lengthText(undefined)).toBe("");
    expect(lengthText(0)).toBe("");
    expect(lengthText(2 * F)).toBe("2");
    expect(lengthText(1.5 * F)).toBe("1.5");
    expect(lengthText(F / 30)).toBe("0.033");
  });

  it("turns typed seconds into the command's duration, 0 for none, null for nonsense", () => {
    expect(lengthArg("")).toBe(0);
    expect(lengthArg("  ")).toBe(0);
    expect(lengthArg("0")).toBe(0);
    expect(lengthArg("1.5")).toBe("1.5s");
    expect(lengthArg("2,25")).toBe("2.25s");
    expect(lengthArg("-1")).toBeNull();
    expect(lengthArg("soon")).toBeNull();
  });
});

describe("markerEdit", () => {
  const marker = { label: "Intro", duration: null };

  it("names only what changed", () => {
    expect(markerEdit(marker, { label: "Intro", color: null, length: "" })).toEqual({});
    expect(markerEdit(marker, { label: " Opening ", color: null, length: "" })).toEqual({
      label: "Opening",
    });
    expect(markerEdit(marker, { label: "Intro", color: "#4a8fe0", length: "2" })).toEqual({
      color: "#4a8fe0",
      duration: "2s",
    });
  });

  it("clears a range marker's length with 0, and refuses a length that is not one", () => {
    const range = { label: "", duration: 2 * F };
    expect(markerEdit(range, { label: "", color: null, length: "2" })).toEqual({});
    expect(markerEdit(range, { label: "", color: null, length: "" })).toEqual({ duration: 0 });
    expect(markerEdit(range, { label: "", color: null, length: "x" })).toBeNull();
  });
});

describe("label colours", () => {
  const root = () => document.documentElement;
  beforeEach(() => {
    root().style.setProperty("--label-red", " #E5534B");
    root().style.setProperty("--label-blue", "#4a8fe0");
  });
  afterEach(() => {
    for (const name of LABEL_COLORS) root().style.removeProperty(`--label-${name}`);
  });

  it("reads a label's value from the tokens as the document stores it", () => {
    expect(labelColorValue("red")).toBe("#e5534b");
    expect(labelColorValue("blue")).toBe("#4a8fe0");
    expect(labelColorValue("pink")).toBeNull(); // no such token in this document
  });

  it("names the label of a stored colour, and none for another colour", () => {
    expect(labelColorOf("#E5534B")).toBe("red");
    expect(labelColorOf("#4a8fe0")).toBe("blue");
    expect(labelColorOf("#f2b134")).toBeNull();
    expect(labelColorOf(null)).toBeNull();
    expect(labelColorOf(undefined)).toBeNull();
  });
});
