// SPDX-License-Identifier: GPL-3.0-or-later
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("./components.css", import.meta.url), "utf8");

const ALLOWED = "@media (prefers-reduced-motion: no-preference)";
/** Reduce motion "Off" (WP-2H.09): the same rules for `data-motion="full"` when the system reduces. */
const KEPT = "@media (prefers-reduced-motion: reduce)";
const KEPT_PREFIX = ':where([data-motion="full"]) ';

/** Splits the stylesheet into the text inside the `marker` media query and the rest. */
function splitMotion(
  source: string,
  marker: string = ALLOWED,
): { guarded: string; unguarded: string } {
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

const { guarded, unguarded: rest } = splitMotion(css);
const { guarded: kept, unguarded } = splitMotion(rest, KEPT);

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

  it('keeps the same motion for data-motion="full" when the system asks to reduce it', () => {
    expect(kept).toContain(KEPT_PREFIX);
    expect(kept.replaceAll(KEPT_PREFIX, "")).toBe(guarded);
  });

  it('makes every transition and animation instant for data-motion="reduce"', () => {
    const rule = /\[data-motion="reduce"\] \*::after \{([^}]*)\}/.exec(unguarded)?.[1] ?? "";
    expect(rule).toContain("transition-property: none !important");
    expect(rule).toContain("animation-name: none !important");
  });

  it("uses the motion tokens, not literal durations", () => {
    expect(guarded).not.toMatch(/\d+m?s\b(?![^{]*\))/);
    expect(guarded).toContain("var(--duration-slow)");
    expect(guarded).toContain("var(--easing-enter)");
  });
});
