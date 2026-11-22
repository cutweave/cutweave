// SPDX-License-Identifier: GPL-3.0-or-later
import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { openStory } from "./helpers.ts";
import { listStories, THEMES } from "./shared.ts";

// axe on every story in both themes; any violation fails (WP-0K.03/0K.04: 0 violations).
// Scoped to the story root: the Storybook iframe shell has no landmarks or title of its own.
// Overlays (menus, dialogs, toasts) render in a portal outside the root, so open ones are
// scanned too; otherwise the floating content would never be checked.
const ROOT = "#storybook-root";
const OVERLAYS = "body > [data-radix-popper-content-wrapper], body > [data-radix-portal]";
// Open, focus-trapping Radix layers.
const MODAL_LAYER = '[role="menu"][data-state="open"], [role="dialog"][data-state="open"]';

/** Scan targets present on the page: the story root plus any open portal overlays. */
async function scanTargets(page: Page): Promise<string[]> {
  const hasOverlay = (await page.locator(OVERLAYS).count()) > 0;
  return hasOverlay ? [ROOT, OVERLAYS] : [ROOT];
}

/**
 * A modal Radix layer (a menu opened on load by a story, as the visual job needs) sets
 * `aria-hidden` on the story root while focus is trapped inside the layer, so the trigger
 * button it contains is unreachable. axe cannot see the trap and reports `aria-hidden-focus`
 * on the root. That is the intended modal pattern, not a defect, so the one result is dropped
 * only when ALL hold: the reported node is exactly the story root, the root is aria-hidden, a
 * modal layer is open, and keyboard focus currently sits inside that layer. The rule stays
 * enabled for every other node and every other story.
 */
async function isTrappedModalArtifact(page: Page, target: string): Promise<boolean> {
  if (target !== ROOT) return false;
  return page.evaluate(
    ({ root, layer }) => {
      const rootEl = document.querySelector(root);
      const open = document.querySelector(layer);
      return (
        rootEl?.getAttribute("aria-hidden") === "true" &&
        open !== null &&
        open.contains(document.activeElement)
      );
    },
    { root: ROOT, layer: MODAL_LAYER },
  );
}

for (const story of listStories()) {
  for (const theme of THEMES) {
    test(`axe: ${story.id} [${theme}]`, async ({ page }) => {
      await openStory(page, story.id, theme);
      // One awaited run per page: a second concurrent axe.run on a frame throws.
      const { violations } = await new AxeBuilder({ page })
        .include(await scanTargets(page))
        .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"])
        .analyze();
      const real: string[] = [];
      for (const v of violations) {
        const nodes: string[] = [];
        for (const n of v.nodes) {
          const target = n.target.join(" ");
          const artifact =
            v.id === "aria-hidden-focus" && (await isTrappedModalArtifact(page, target));
          if (!artifact) nodes.push(target);
        }
        if (nodes.length) {
          real.push(`${v.id} (${v.impact}): ${v.help}\n${nodes.map((t) => `    ${t}`).join("\n")}`);
        }
      }
      expect(real, real.join("\n")).toEqual([]);
    });
  }
}
