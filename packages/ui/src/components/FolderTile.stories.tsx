// SPDX-License-Identifier: GPL-3.0-or-later
import { FolderTile } from "./FolderTile.tsx";

export default { title: "Media/FolderTile", component: FolderTile, tags: ["autodocs"] };

const grid = {
  padding: "var(--space-4)",
  display: "grid",
  gridTemplateColumns: "repeat(auto-fill, minmax(var(--layer-width-sm), 1fr))",
  gap: "var(--space-3)",
  background: "var(--surface-1)",
} as const;

/** Default, selected and a drop target (something dragged over it). Renaming is the InlineRename story. */
export const States = {
  render: () => (
    <div role="listbox" aria-label="Folders" style={grid}>
      <FolderTile name="Interviews" countLabel="12 items" active kindLabel="Folder" />
      <FolderTile name="B-roll" countLabel="1 item" selected kindLabel="Folder" />
      <FolderTile name="Music" countLabel="Empty" dropTarget kindLabel="Folder" />
    </div>
  ),
};

/** In search results across the whole bin the path sits under the name. */
export const WithPath = {
  render: () => (
    <div role="listbox" aria-label="Folders" style={grid}>
      <FolderTile name="Interviews" path="Media / Day 1" countLabel="4 items" kindLabel="Folder" />
    </div>
  ),
};
