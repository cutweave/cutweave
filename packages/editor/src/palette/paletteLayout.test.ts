// SPDX-License-Identifier: GPL-3.0-or-later
// The palette's size (#160): at most half the window tall, the list scrolling inside it. jsdom
// lays nothing out, so this reads the declarations the stylesheet gives the palette.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("../app.css", import.meta.url), "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  "",
);

/** The declarations of the rule whose selector list is exactly `selector`. */
function declarations(selector: string): Map<string, string> {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const rule = new RegExp(`(?:^|})\\s*${escaped}\\s*\\{([^}]*)\\}`).exec(css);
  if (rule === null) throw new Error(`no rule for ${selector}`);
  const out = new Map<string, string>();
  for (const part of (rule[1] ?? "").split(";")) {
    const colon = part.indexOf(":");
    if (colon > 0) out.set(part.slice(0, colon).trim(), part.slice(colon + 1).trim());
  }
  return out;
}

describe("command palette size", () => {
  const dialog = declarations(".cw-dialog.cw-dialog--palette");
  const list = declarations(".cw-palette__list");

  it("is at most half the window tall", () => {
    expect(dialog.get("max-height")).toBe("50vh");
  });

  it("stays anchored near the top and centred across", () => {
    expect(dialog.get("top")).toBe("var(--space-8)");
    expect(dialog.get("transform")).toBe("translateX(-50%)");
  });

  it("scrolls its list inside instead of growing", () => {
    expect(list.get("min-height")).toBe("0");
    expect(list.get("overflow-y")).toBe("auto");
  });
});
