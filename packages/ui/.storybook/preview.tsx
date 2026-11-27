// SPDX-License-Identifier: GPL-3.0-or-later
import type { Decorator, Preview } from "@storybook/react-vite";
import "../src/tokens/tokens.css";
import "../src/fonts/fonts.css";
// Component styles: stories import components directly, so the package entry (src/index.ts) is not loaded here.
import "../src/components/components.css";
import "../src/components/scrollbars.css";
import "./preview.css";

export const THEMES = ["dark", "light"] as const;
export type Theme = (typeof THEMES)[number];

/** Families a story may use; loaded before render so screenshots never catch a fallback face. */
const FONT_LOADS = [
  'normal 400 13px "Instrument Sans"',
  'normal 600 13px "Instrument Sans"',
  'normal 700 28px "Bricolage Grotesque"',
  'normal 500 13px "JetBrains Mono"',
];

const withTheme: Decorator = (Story, context) => {
  const theme: Theme = context.globals.theme === "light" ? "light" : "dark";
  document.documentElement.dataset.theme = theme;
  // Standard is `compact`; Comfortable is the larger size tokens (design/tokens.json `density`).
  document.documentElement.dataset.density =
    context.globals.density === "comfortable" ? "comfortable" : "compact";
  // `.body` (13px / 20px Instrument Sans) comes from the generated type classes in tokens.css.
  document.body.classList.add("body");
  return <Story />;
};

const preview: Preview = {
  decorators: [withTheme],
  // Dark is the default theme (docs/07-design-system.md). Tests pass `&globals=theme:light`.
  initialGlobals: { theme: "dark", density: "compact" },
  globalTypes: {
    theme: {
      description: "Theme (sets data-theme on the document root)",
      toolbar: {
        title: "Theme",
        icon: "circlehollow",
        dynamicTitle: true,
        items: [
          { value: "dark", title: "Dark (default)" },
          { value: "light", title: "Light" },
        ],
      },
    },
    density: {
      description:
        "Density (sets data-density on the document root). Standard is compact: controls are 24/28/32 px with a 24 px minimum hit target. Comfortable makes controls, bars, tracks and padding larger (Settings > Appearance > Density).",
      toolbar: {
        title: "Density",
        icon: "ruler",
        dynamicTitle: true,
        items: [
          { value: "compact", title: "Standard (24/28/32 px controls)" },
          { value: "comfortable", title: "Comfortable (28/32/36 px controls)" },
        ],
      },
    },
  },
  loaders: [
    async () => {
      await Promise.all(FONT_LOADS.map((f) => document.fonts.load(f, "Aa0")));
      await document.fonts.ready;
      return {};
    },
  ],
  parameters: {
    layout: "padded",
    backgrounds: { disabled: true },
    controls: { expanded: true },
    a11y: { test: "error" },
  },
};

export default preview;
