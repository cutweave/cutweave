// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import {
  type ColorTokenName,
  colors,
  cssVar,
  radius,
  shadow,
  spacing,
  themes,
  typeStyles,
} from "./index.ts";

describe("@cutweave/tokens", () => {
  it("lists dark first, then light", () => {
    expect(themes).toEqual(["dark", "light"]);
  });

  it("has the same color names in every theme", () => {
    expect(Object.keys(colors.light)).toEqual(Object.keys(colors.dark));
  });

  it("resolves aliases and keeps alpha", () => {
    expect(colors.dark.playhead).toBe(colors.dark.marigold);
    expect(colors.dark["marigold-subtle"]).toMatch(/^#[0-9a-f]{8}$/);
  });

  it("types token names as literal unions", () => {
    const name: ColorTokenName = "surface-0";
    expect(colors.light[name]).toBe("#e7e7e7");
    // @ts-expect-error unknown token names do not type-check
    const bad: ColorTokenName = "surface-9";
    expect(bad).toBe("surface-9");
  });

  it("exposes the other families", () => {
    expect(spacing["space-1"]).toBe("4px");
    expect(radius["radius-sm"]).toBe("4px");
    expect(shadow.dark["shadow-popover"]).toContain("rgba(");
    expect(typeStyles.body.fontSize).toBe("13px");
    expect(cssVar("surface-0")).toBe("var(--surface-0)");
  });
});
