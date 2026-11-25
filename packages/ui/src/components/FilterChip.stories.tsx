// SPDX-License-Identifier: GPL-3.0-or-later
import { FilterChip } from "./FilterChip.tsx";

export default { title: "Inputs/FilterChip", component: FilterChip, tags: ["autodocs"] };

const noop = () => {};

const row = {
  padding: "var(--space-4)",
  display: "flex",
  flexWrap: "wrap",
  gap: "var(--space-2)",
  alignItems: "center",
  background: "var(--surface-1)",
} as const;

/** The bin's active filters: each chip names one and removes it with its button. */
export const ActiveFilters = {
  render: () => (
    <div style={row}>
      <FilterChip label="Video" removeLabel="Remove filter Video" onRemove={noop} />
      <FilterChip label="Unused" removeLabel="Remove filter Unused" onRemove={noop} />
      <FilterChip
        label="Name contains “interview”"
        removeLabel="Remove filter Name contains “interview”"
        onRemove={noop}
      />
    </div>
  ),
};
