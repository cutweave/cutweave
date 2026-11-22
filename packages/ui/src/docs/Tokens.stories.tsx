// SPDX-License-Identifier: GPL-3.0-or-later
import type { Meta, StoryObj } from "@storybook/react-vite";
import tokens from "../../../../design/tokens.json";

// Swatches paint `var(--name)`, so the page follows the toolbar theme; the hex values printed
// next to each swatch are the documented values from design/tokens.json (the source of truth).

interface ColorToken {
  name: string;
  value: { dark: string; light: string };
  usage: string;
}
interface TypeStyle {
  name: string;
  fontSize: string;
  lineHeight: string;
  fontWeight: number;
  sample: string;
  usage: string;
}
interface TypeGroup {
  name: string;
  styles: TypeStyle[];
}
interface ScalarToken {
  name: string;
  value: string;
  usage?: string;
}

const colors = tokens.color.tokens as ColorToken[];
const typeGroups = tokens.type.groups as TypeGroup[];
const spacing = tokens.spacing.tokens as ScalarToken[];
const radius = tokens.radius.tokens as ScalarToken[];
const sizes = tokens.size.tokens as ScalarToken[];

/** Color tokens grouped by name prefix, in file order. */
const COLOR_GROUPS: [string, (n: string) => boolean][] = [
  ["Surfaces and lanes", (n) => n.startsWith("surface-") || n.startsWith("timeline-")],
  ["Borders", (n) => n.startsWith("border-")],
  ["Text", (n) => n.startsWith("text-")],
  [
    "Marigold (here) and focus",
    (n) => n.includes("marigold") || n === "focus-ring" || n === "playhead",
  ],
  ["Agent blue (AI and agent)", (n) => n.includes("agent")],
  ["Clip families", (n) => n.startsWith("clip-")],
  ["Status", (n) => ["success", "warning", "danger", "danger-hover", "on-danger"].includes(n)],
  ["Overlays", (n) => n === "scrim" || n.startsWith("overlay-")],
];

const h2 = {
  margin: "var(--space-6) 0 var(--space-2)",
  color: "var(--text-primary)",
} as const;
const note = { color: "var(--text-secondary)", margin: "0 0 var(--space-3)" } as const;
const row = {
  display: "grid",
  gridTemplateColumns: "var(--space-12) minmax(0, 14rem) minmax(0, 1fr)",
  alignItems: "center",
  gap: "var(--space-3)",
  padding: "var(--space-1-5) var(--space-3)",
  background: "var(--surface-2)",
  border: "var(--border-thin) solid var(--border-subtle)",
  borderRadius: "var(--radius-md)",
} as const;
const swatch = {
  height: "var(--control-lg)",
  borderRadius: "var(--radius-sm)",
  border: "var(--border-thin) solid var(--border-default)",
} as const;
const stack = { display: "grid", gap: "var(--space-1)", margin: 0, padding: 0, listStyle: "none" };

function Colors() {
  return (
    <div>
      <p className="body" style={note}>
        Colors switch with the Theme toolbar. Every text/background pair named in a usage note is
        checked for WCAG AA contrast in both themes by the token pipeline.
      </p>
      {COLOR_GROUPS.map(([title, match]) => (
        <section key={title}>
          <h2 className="heading" style={h2}>
            {title}
          </h2>
          <ul style={stack}>
            {colors
              .filter((c) => match(c.name))
              .map((c) => (
                <li key={c.name} style={row} data-token={c.name}>
                  <span style={{ ...swatch, background: `var(--${c.name})` }} />
                  <span>
                    <span className="numeric" style={{ color: "var(--text-primary)" }}>
                      --{c.name}
                    </span>
                    <br />
                    <span className="caption" style={{ color: "var(--text-muted)" }}>
                      dark {c.value.dark} / light {c.value.light}
                    </span>
                  </span>
                  <span className="label" style={{ color: "var(--text-secondary)" }}>
                    {c.usage}
                  </span>
                </li>
              ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

function Type() {
  return (
    <div>
      <p className="body" style={note}>
        Instrument Sans for the interface (13 px base), Bricolage Grotesque for brand moments only,
        JetBrains Mono for timecode and numbers. All three are bundled for offline use. Apply a
        style with its class, for example <code className="numeric">className="heading"</code>.
      </p>
      {typeGroups.map((g) => (
        <section key={g.name}>
          <h2 className="heading" style={h2}>
            {g.name}
          </h2>
          <ul style={stack}>
            {g.styles.map((s) => (
              <li
                key={s.name}
                style={{ ...row, gridTemplateColumns: "minmax(0, 14rem) minmax(0, 1fr)" }}
                data-type-style={s.name}
              >
                <span>
                  <span className="numeric" style={{ color: "var(--text-primary)" }}>
                    .{s.name}
                  </span>
                  <br />
                  <span className="caption" style={{ color: "var(--text-muted)" }}>
                    {s.fontSize} / {s.lineHeight} / {s.fontWeight}
                  </span>
                </span>
                <span>
                  <span className={s.name} style={{ color: "var(--text-primary)" }}>
                    {s.sample}
                  </span>
                  <br />
                  <span className="label" style={{ color: "var(--text-secondary)" }}>
                    {s.usage}
                  </span>
                </span>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

function Scale({ title, items, bar }: { title: string; items: ScalarToken[]; bar?: boolean }) {
  return (
    <section>
      <h2 className="heading" style={h2}>
        {title}
      </h2>
      <ul style={stack}>
        {items.map((t) => (
          <li
            key={t.name}
            style={{ ...row, gridTemplateColumns: "minmax(0, 14rem) minmax(0, 1fr)" }}
            data-token={t.name}
          >
            <span>
              <span className="numeric" style={{ color: "var(--text-primary)" }}>
                --{t.name}
              </span>
              <br />
              <span className="caption" style={{ color: "var(--text-muted)" }}>
                {t.value}
              </span>
            </span>
            <span className="label" style={{ color: "var(--text-secondary)" }}>
              {bar ? (
                <span
                  aria-hidden="true"
                  style={{
                    display: "inline-block",
                    width: `var(--${t.name})`,
                    height: "var(--space-2)",
                    marginRight: "var(--space-3)",
                    background: "var(--marigold)",
                    borderRadius: "var(--radius-xs)",
                    verticalAlign: "middle",
                  }}
                />
              ) : null}
              {t.usage ?? ""}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function Density() {
  return (
    <div>
      <p className="body" style={note}>
        One density ships: compact. Controls are 24 / 28 / 32 px (--control-sm / md / lg) with a 24
        px minimum hit target (--hit-min, WCAG 2.2). The Density toolbar item sets
        <code className="numeric"> data-density="compact"</code> on the root so components can opt
        in to a second density later; none is planned before P7.
      </p>
      <ul
        style={{ ...stack, gridTemplateColumns: "repeat(3, max-content)", gap: "var(--space-3)" }}
      >
        {(["sm", "md", "lg"] as const).map((s) => (
          <li
            key={s}
            className="label"
            style={{
              display: "flex",
              alignItems: "center",
              height: `var(--control-${s})`,
              padding: "0 var(--space-3)",
              background: "var(--surface-3)",
              color: "var(--text-primary)",
              borderRadius: "var(--radius-sm)",
              border: "var(--border-thin) solid var(--border-control)",
            }}
          >
            control-{s}
          </li>
        ))}
      </ul>
    </div>
  );
}

const meta = {
  title: "Foundations/Tokens",
  parameters: { layout: "padded" },
} satisfies Meta;

export default meta;
type Story = StoryObj<typeof meta>;

/** Color swatches for the active theme, each with its documented usage. */
export const Color: Story = { render: () => <Colors /> };

/** Type styles, rendered with the bundled fonts. */
export const Typography: Story = { render: () => <Type /> };

/** Spacing, radius and size scales. */
export const SpacingAndSize: Story = {
  render: () => (
    <div>
      <Scale title="Spacing (4 px grid)" items={spacing} bar />
      <Scale title="Radius" items={radius} />
      <Scale title="Control and layout sizes" items={sizes} />
    </div>
  ),
};

/** Density note. */
export const DensityNote: Story = {
  name: "Density",
  render: () => <Density />,
};
