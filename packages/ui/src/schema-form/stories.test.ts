// SPDX-License-Identifier: GPL-3.0-or-later
/// <reference types="vite/client" />
// The form is a library component, so it has a story like every component in packages/ui (the
// components folder enforces the same rule file by file).
import { describe, expect, it } from "vitest";

const files = Object.keys(import.meta.glob("./*.tsx"));

describe("Storybook coverage of the schema form", () => {
  it("has a story for the form and no component without one", () => {
    const stories = new Set(files.filter((f) => f.endsWith(".stories.tsx")));
    expect(stories.has("./SchemaForm.stories.tsx")).toBe(true);
    const components = files.filter((f) => !f.endsWith(".stories.tsx"));
    // SchemaForm.tsx is the component; controls.tsx holds its widgets, shown in its Gallery story.
    expect(components.sort()).toEqual(["./SchemaForm.tsx", "./controls.tsx"]);
  });
});
