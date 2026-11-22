// SPDX-License-Identifier: GPL-3.0-or-later
import type { StorybookConfig } from "@storybook/react-vite";

// Picks up every `*.stories.tsx` under src/ (components land in src/components/, docs pages in
// src/docs/, the icon gallery beside the icons).
const config: StorybookConfig = {
  stories: ["../src/**/*.stories.@(ts|tsx)"],
  addons: ["@storybook/addon-a11y"],
  framework: { name: "@storybook/react-vite", options: {} },
  core: { disableTelemetry: true, disableWhatsNewNotifications: true },
  typescript: { reactDocgen: false },
};

export default config;
