// SPDX-License-Identifier: GPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { openStory } from "./helpers.ts";
import { BASE_URL } from "./shared.ts";

// WP-0K.02 (deferred here): with the network blocked, every bundled family still loads.
// Any request that leaves the local static server is aborted and recorded; none may be made.
const FAMILIES = [
  { family: "Instrument Sans", weights: [400, 600] },
  { family: "Bricolage Grotesque", weights: [700] },
  { family: "JetBrains Mono", weights: [400, 500] },
];

test("bundled fonts load and check() is true with the network blocked", async ({ page }) => {
  const blocked: string[] = [];
  await page.route(
    (url) => url.origin !== BASE_URL && url.protocol.startsWith("http"),
    (route) => {
      blocked.push(route.request().url());
      return route.abort("blockedbyclient");
    },
  );
  await openStory(page, "foundations-tokens--typography", "dark");

  for (const { family, weights } of FAMILIES) {
    for (const weight of weights) {
      const spec = `${weight} 12px "${family}"`;
      const result = await page.evaluate(async (s) => {
        await document.fonts.load(s, "Aa0");
        return {
          check: document.fonts.check(s, "Aa0"),
          // check() is vacuously true for an unknown family: require a loaded face as well.
          loaded: [...document.fonts].filter((f) => f.status === "loaded").map((f) => f.family),
        };
      }, spec);
      expect(result.check, `document.fonts.check(${spec})`).toBe(true);
      expect(result.loaded.map((f) => f.replaceAll('"', ""))).toContain(family);
    }
  }
  expect(blocked, "fonts must not touch the network").toEqual([]);
});
