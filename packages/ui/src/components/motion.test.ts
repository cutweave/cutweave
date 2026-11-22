// SPDX-License-Identifier: GPL-3.0-or-later
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("./components.css", import.meta.url), "utf8");

/** Splits the stylesheet into the text inside `prefers-reduced-motion: no-preference` and the rest. */
function splitMotion(source: string): { guarded: string; unguarded: string } {
  const marker = "@media (prefers-reduced-motion: no-preference)";
  let guarded = "";
  let unguarded = "";
  let rest = source;
  for (;;) {
    const at = rest.indexOf(marker);
    if (at === -1) break;
    unguarded += rest.slice(0, at);
    let depth = 0;
    let i = rest.indexOf("{", at);
    const start = i + 1;
    for (; i < rest.length; i++) {
      if (rest[i] === "{") depth++;
      else if (rest[i] === "}" && --depth === 0) break;
    }
    guarded += rest.slice(start, i);
    rest = rest.slice(i + 1);
  }
  return { guarded, unguarded: unguarded + rest };
}

const { guarded, unguarded } = splitMotion(css);

describe("reduced motion", () => {
  it("never animates or transitions outside the no-preference media query", () => {
    expect(unguarded).not.toMatch(/\banimation\s*:/);
    expect(unguarded).not.toMatch(/\btransition\s*:/);
    expect(unguarded).not.toMatch(/@keyframes/);
  });

  it.each([".cw-toast", ".cw-menu", ".cw-dialog", ".cw-tooltip"])(
    "%s animates only when motion is allowed",
    (selector) => {
      expect(guarded).toContain(`${selector}[data-state`);
    },
  );

  it("uses the motion tokens, not literal durations", () => {
    expect(guarded).not.toMatch(/\d+m?s\b(?![^{]*\))/);
    expect(guarded).toContain("var(--duration-slow)");
    expect(guarded).toContain("var(--easing-enter)");
  });
});
