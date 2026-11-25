// SPDX-License-Identifier: GPL-3.0-or-later
import { ProgressBar } from "./ProgressBar.tsx";

export default { title: "Feedback/ProgressBar", component: ProgressBar, tags: ["autodocs"] };

const column = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-3)",
  width: "var(--layer-width-md)",
  background: "var(--surface-1)",
} as const;

/** Start, partway and done: the bar clamps to 0–100 and announces its value. */
export const Values = {
  render: () => (
    <div style={column}>
      <ProgressBar value={0} label="Export: queued" />
      <ProgressBar value={35} label="Export: 35%" />
      <ProgressBar value={100} label="Export: done" />
    </div>
  ),
};
