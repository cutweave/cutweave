// SPDX-License-Identifier: GPL-3.0-or-later
/// <reference types="vite/client" />
// Every component in packages/ui has a Storybook story, so Storybook and the visual baselines
// never fall behind the library (owner request, 2026-11-25).
import { describe, expect, it } from "vitest";

const files = Object.keys(import.meta.glob("./*.tsx"));
const stories = new Set(files.filter((f) => f.endsWith(".stories.tsx")));
const components = files.filter((f) => !f.endsWith(".stories.tsx") && !f.endsWith(".test.tsx"));

describe("Storybook coverage", () => {
  it("finds the components", () => {
    expect(components.length).toBeGreaterThan(20);
  });
  it.each(components)("%s has a story", (file) => {
    expect(stories.has(file.replace(/\.tsx$/, ".stories.tsx"))).toBe(true);
  });
});
