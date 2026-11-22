// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { createElement as h } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { StatusChip, type StatusTone } from "./StatusChip.tsx";

afterEach(cleanup);

const TONES: [StatusTone, string][] = [
  ["success", "check"],
  ["warning", "triangle-alert"],
  ["danger", "circle-alert"],
  ["neutral", "info"],
];

describe("StatusChip", () => {
  it.each(TONES)("%s tone always shows an icon and a word", (tone, icon) => {
    render(h(StatusChip, { tone, children: "Some status" }));
    const chip = screen.getByText("Some status");
    expect(chip.getAttribute("data-tone")).toBe(tone);
    const svg = chip.querySelector("svg");
    expect(svg?.getAttribute("data-icon")).toBe(icon);
    expect(svg?.getAttribute("aria-hidden")).toBe("true");
  });

  it("is static: not focusable and not interactive", () => {
    render(h(StatusChip, { tone: "success", children: "Exported" }));
    const chip = screen.getByText("Exported");
    expect(chip.hasAttribute("tabindex")).toBe(false);
    expect(chip.getAttribute("role")).toBeNull();
  });
});
