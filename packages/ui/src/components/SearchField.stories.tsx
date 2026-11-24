// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { FilterChip } from "./FilterChip.tsx";
import { IconButton } from "./IconButton.tsx";
import { Menu, type MenuEntry } from "./Menu.tsx";
import { SearchField } from "./SearchField.tsx";
import { Tooltip } from "./Tooltip.tsx";

export default { title: "Inputs/SearchField", component: SearchField, tags: ["autodocs"] };

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-3)",
  width: "var(--layer-width-md)",
} as const;

function Field({ initial = "", count }: { initial?: string; count?: string }) {
  const [value, setValue] = useState(initial);
  return (
    <SearchField
      value={value}
      onChange={setValue}
      label="Search media"
      clearLabel="Clear search"
      {...(count === undefined ? {} : { count })}
    />
  );
}

export const Empty = {
  render: () => (
    <div style={col}>
      <Field />
    </div>
  ),
};

export const WithText = {
  render: () => (
    <div style={col}>
      <Field initial="beach" />
    </div>
  ),
};

export const WithCount = {
  render: () => (
    <div style={col}>
      <Field initial="beach" count="12 of 340" />
    </div>
  ),
};

const FILTER_ITEMS: MenuEntry[] = [
  { kind: "label", id: "kind", label: "Kind" },
  { id: "video", label: "Video", checked: false, keepOpen: true },
  { id: "audio", label: "Audio", checked: true, keepOpen: true },
  { id: "image", label: "Image", checked: false, keepOpen: true },
  { kind: "separator", id: "s1" },
  { kind: "label", id: "use", label: "Use" },
  { id: "any", label: "Any", checked: false, choice: "radio", keepOpen: true },
  { id: "unused", label: "Not used", checked: true, choice: "radio", keepOpen: true },
];

/**
 * The media bin's toolbar: icon buttons with a count badge, the search field with its result
 * count and the chips of the active filters (WP-1M.12). The Filters menu is open to show the
 * checked rows.
 */
export const BinToolbarWithChips = {
  render: () => (
    <div style={{ ...col, width: "var(--layer-width-lg)" }}>
      <div style={{ display: "flex", gap: "var(--space-1)", alignItems: "center" }}>
        <Tooltip label="Import">
          <IconButton size="sm" icon="upload" label="Import" />
        </Tooltip>
        <Tooltip label="Remove from project">
          <IconButton size="sm" icon="trash-2" label="Remove from project" disabled />
        </Tooltip>
        <div style={{ flex: 1 }}>
          <Field initial="beach" count="2 of 14" />
        </div>
        <Menu mode="dropdown" defaultOpen items={FILTER_ITEMS} aria-label="Filters">
          <IconButton size="sm" icon="sliders-horizontal" label="Filters, 2 active" badge={2} />
        </Menu>
        <IconButton size="sm" icon="arrow-up-down" label="Sort" />
      </div>
      <div style={{ display: "flex", gap: "var(--space-1)", flexWrap: "wrap" }}>
        <FilterChip label="Audio" removeLabel="Remove filter Audio" onRemove={() => {}} />
        <FilterChip label="Not used" removeLabel="Remove filter Not used" onRemove={() => {}} />
      </div>
    </div>
  ),
};
