// SPDX-License-Identifier: GPL-3.0-or-later
// The global scrollbar (scrollbars.css): every scrollable area gets it without a class.

export default { title: "Structure/Scrollbar", tags: ["autodocs"] };

const frame = {
  padding: "var(--space-4)",
  background: "var(--surface-0)",
} as const;

const box = {
  width: "var(--layer-width-sm)",
  height: "var(--layer-width-sm)",
  overflow: "auto",
  background: "var(--surface-1)",
  border: "var(--border-thin) solid var(--border-subtle)",
  borderRadius: "var(--radius-md)",
} as const;

const rows = Array.from({ length: 24 }, (_, i) => `Clip ${String(i + 1).padStart(2, "0")}`);

/** Vertical overflow, as in the media bin and the inspector. A scroll area is a focusable,
 * labelled region so keyboard users can scroll it (axe: scrollable-region-focusable). */
export const Vertical = () => (
  <div style={frame}>
    {/* biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll area must take focus so the keyboard can scroll it (axe scrollable-region-focusable). */}
    <section style={box} tabIndex={0} aria-label="Clips">
      {rows.map((r) => (
        <div key={r} className="cw-row" style={{ padding: "var(--space-1-5) var(--space-3)" }}>
          <span className="cw-row__label label">{r}</span>
        </div>
      ))}
    </section>
  </div>
);

/** Both axes overflow, as in the timeline: the corner stays transparent. */
export const BothAxes = () => (
  <div style={frame}>
    {/* biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll area must take focus so the keyboard can scroll it (axe scrollable-region-focusable). */}
    <section style={box} tabIndex={0} aria-label="Clips">
      <div style={{ width: "calc(var(--layer-width-md) * 3)" }}>
        {rows.map((r) => (
          <div key={r} className="cw-row" style={{ padding: "var(--space-1-5) var(--space-3)" }}>
            <span className="cw-row__label label">{r}</span>
          </div>
        ))}
      </div>
    </section>
  </div>
);
