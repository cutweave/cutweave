// SPDX-License-Identifier: GPL-3.0-or-later
import { CostChip } from "./CostChip.tsx";

export default { title: "AI & agents/CostChip", component: CostChip, tags: ["autodocs"] };

const row = {
  padding: "var(--space-4)",
  display: "flex",
  flexWrap: "wrap",
  gap: "var(--space-3)",
  alignItems: "center",
  background: "var(--surface-2)",
} as const;

export const States = {
  render: () => (
    <div style={row}>
      <CostChip state="estimate" amount={0.4} />
      <CostChip state="free" />
      <CostChip state="over-budget" amount={4.8} />
      <CostChip state="approval" />
      <CostChip state="approval" amount={2.1} />
      <CostChip state="estimating" />
    </div>
  ),
};
