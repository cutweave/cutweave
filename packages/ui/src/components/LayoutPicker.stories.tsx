// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { type LayoutOption, LayoutPicker } from "./LayoutPicker.tsx";

export default { title: "Actions/LayoutPicker", component: LayoutPicker, tags: ["autodocs"] };

const pad = { padding: "var(--space-4)", display: "flex", gap: "var(--space-3)" };

const TWO: LayoutOption[] = [
  {
    id: "side_by_side",
    label: "Side by side",
    cells: [
      { x: 0, y: 0, w: 0.5, h: 1 },
      { x: 0.5, y: 0, w: 0.5, h: 1 },
    ],
  },
  {
    id: "top_bottom",
    label: "Top and bottom",
    cells: [
      { x: 0, y: 0, w: 1, h: 0.5 },
      { x: 0, y: 0.5, w: 1, h: 0.5 },
    ],
  },
  {
    id: "pip_top_right",
    label: "Picture in picture, top right",
    cells: [
      { x: 0.65, y: 0.05, w: 0.3, h: 0.3 },
      { x: 0, y: 0, w: 1, h: 1 },
    ],
  },
];

function Demo({ disabled = false }: { disabled?: boolean }) {
  const [last, setLast] = useState("none");
  return (
    <div style={pad}>
      <LayoutPicker aria-label="Layout" options={TWO} disabled={disabled} onPick={setLast} />
      <span className="caption" data-testid="picked">
        {last}
      </span>
    </div>
  );
}

export const Default = { render: () => <Demo /> };
export const Disabled = { render: () => <Demo disabled /> };
