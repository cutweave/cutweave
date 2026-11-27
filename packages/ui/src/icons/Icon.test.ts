// SPDX-License-Identifier: GPL-3.0-or-later
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  CUSTOM_ICON_NAMES,
  ICON_NAMES,
  ICON_SIZE_TOKENS,
  Icon,
  type IconName,
  type IconProps,
} from "./index.ts";

const render = (props: IconProps) => renderToStaticMarkup(createElement(Icon, props));

describe("<Icon>", () => {
  it("renders an svg for every registered name", () => {
    for (const name of ICON_NAMES) {
      const html = render({ name });
      expect(html, name).toMatch(/^<svg /);
      expect(html, name).toContain(`data-icon="${name}"`);
    }
  });

  it("includes every custom editor glyph and keeps names unique", () => {
    const expected: IconName[] = [
      "razor",
      "ripple",
      "roll",
      "slip",
      "slide",
      "magnet",
      "keyframe-diamond",
      "keyframe-diamond-filled",
      "keyframe-diamond-half",
      "ease-hold",
      "ease-linear",
      "ease-bezier",
      "ease-spring",
      "mark-in",
      "mark-out",
      "proposal",
    ];
    expect([...CUSTOM_ICON_NAMES].sort()).toEqual([...expected].sort());
    expect(ICON_NAMES).toContain("sparkles");
    expect(new Set(ICON_NAMES).size).toBe(ICON_NAMES.length);
  });

  it("maps sizes to the icon tokens, md by default", () => {
    expect(render({ name: "play" })).toContain("width:var(--icon-md)");
    for (const size of ["sm", "md", "lg"] as const) {
      const html = render({ name: "play", size });
      expect(html).toContain(`width:${ICON_SIZE_TOKENS[size]}`);
      expect(html).toContain(`height:${ICON_SIZE_TOKENS[size]}`);
      expect(html).toContain(`data-size="${size}"`);
    }
    expect(ICON_SIZE_TOKENS).toEqual({
      sm: "var(--icon-sm)",
      md: "var(--icon-md)",
      lg: "var(--icon-lg)",
    });
  });

  it("uses stroke 1.75 and currentColor on the 24-unit grid (Lucide and custom)", () => {
    for (const name of ["play", "razor", "keyframe-diamond-half"] as const) {
      const html = render({ name });
      expect(html, name).toContain('stroke-width="1.75"');
      expect(html, name).toContain('stroke="currentColor"');
      expect(html, name).toContain('viewBox="0 0 24 24"');
    }
  });

  it("is aria-hidden by default", () => {
    const html = render({ name: "scissors" });
    expect(html).toContain('aria-hidden="true"');
    expect(html).not.toContain("aria-label");
    expect(html).not.toContain('role="img"');
  });

  it("is labelled, and not hidden, when label is passed", () => {
    const html = render({ name: "razor", label: "Razor tool" });
    expect(html).toContain('role="img"');
    expect(html).toContain('aria-label="Razor tool"');
    expect(html).not.toContain("aria-hidden");
  });

  it("passes className and merges style", () => {
    const html = render({ name: "play", className: "x", style: { color: "red" } });
    expect(html).toContain("x");
    expect(html).toContain("color:red");
    expect(html).toContain("width:var(--icon-md)");
  });
});
