// SPDX-License-Identifier: GPL-3.0-or-later
import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import { openStory } from "./helpers.ts";
import { listStories, THEMES } from "./shared.ts";

// axe on every story in both themes; any violation fails (WP-0K.03/0K.04: 0 violations).
// Scoped to the story root: the Storybook iframe shell has no landmarks or title of its own.
for (const story of listStories()) {
  for (const theme of THEMES) {
    test(`axe: ${story.id} [${theme}]`, async ({ page }) => {
      await openStory(page, story.id, theme);
      const { violations } = await new AxeBuilder({ page })
        .include("#storybook-root")
        .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"])
        .analyze();
      const summary = violations.map(
        (v) =>
          `${v.id} (${v.impact}): ${v.help}\n` +
          v.nodes.map((n) => `    ${n.target.join(" ")}`).join("\n"),
      );
      expect(summary, summary.join("\n")).toEqual([]);
    });
  }
}
