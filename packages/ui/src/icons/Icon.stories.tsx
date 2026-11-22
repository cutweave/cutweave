// SPDX-License-Identifier: GPL-3.0-or-later
import type { Meta, StoryObj } from "@storybook/react-vite";
import { CUSTOM_ICON_NAMES, ICON_NAMES, Icon, type IconName, type IconSize } from "./index.ts";

const SIZES: readonly IconSize[] = ["sm", "md", "lg"];
const CUSTOM = new Set<string>(CUSTOM_ICON_NAMES);

const grid = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fill, minmax(var(--layer-width-sm), 1fr))",
  gap: "var(--space-2)",
  margin: 0,
  padding: 0,
  listStyle: "none",
} as const;

const cell = {
  display: "flex",
  alignItems: "center",
  gap: "var(--space-3)",
  padding: "var(--space-2) var(--space-3)",
  background: "var(--surface-2)",
  border: "var(--border-thin) solid var(--border-subtle)",
  borderRadius: "var(--radius-md)",
} as const;

function IconGrid({ names }: { names: readonly IconName[] }) {
  return (
    <ul style={grid}>
      {names.map((name) => (
        <li key={name} style={cell} data-testid={`icon-${name}`}>
          <span style={{ display: "flex", alignItems: "center", gap: "var(--space-2)" }}>
            {SIZES.map((size) => (
              <Icon key={size} name={name} size={size} />
            ))}
          </span>
          <span className="numeric" style={{ color: "var(--text-secondary)" }}>
            {name}
            {CUSTOM.has(name) ? " (custom)" : ""}
          </span>
        </li>
      ))}
    </ul>
  );
}

const meta = {
  title: "Foundations/Icons",
  parameters: {
    docs: {
      description: {
        component:
          "Every icon in the set at icon-sm (14), icon-md (16) and icon-lg (20) px, stroke 1.75 on the 24-unit grid. Icons take `currentColor`.",
      },
    },
  },
} satisfies Meta;

export default meta;
type Story = StoryObj<typeof meta>;

/** All Lucide icons used by Cutweave plus the custom editor glyphs. */
export const Gallery: Story = {
  render: () => <IconGrid names={ICON_NAMES} />,
};

/** Custom glyphs only (editor-specific marks that Lucide does not provide). */
export const CustomGlyphs: Story = {
  render: () => <IconGrid names={CUSTOM_ICON_NAMES} />,
};

/** The same icon in each theme-relevant text role, to check `currentColor` inheritance. */
export const Colors: Story = {
  render: () => (
    <div style={{ display: "flex", gap: "var(--space-4)" }}>
      {(
        [
          "--text-primary",
          "--text-secondary",
          "--marigold-text",
          "--agent-text",
          "--danger",
        ] as const
      ).map((token) => (
        <span
          key={token}
          style={{ color: `var(${token})`, display: "flex", gap: "var(--space-1)" }}
        >
          <Icon name={ICON_NAMES[0] as IconName} size="lg" />
          <span className="caption">{token}</span>
        </span>
      ))}
    </div>
  ),
};
