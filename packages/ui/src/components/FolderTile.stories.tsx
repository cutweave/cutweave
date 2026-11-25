// SPDX-License-Identifier: GPL-3.0-or-later
import { FolderTile } from "./FolderTile.tsx";

export default { title: "Media/FolderTile", component: FolderTile, tags: ["autodocs"] };

const noop = () => {};

const grid = {
  padding: "var(--space-4)",
  display: "grid",
  gridTemplateColumns: "repeat(auto-fill, minmax(var(--layer-width-sm), 1fr))",
  gap: "var(--space-3)",
  background: "var(--surface-1)",
} as const;

/** Default, selected, a drop target (something dragged over it) and being renamed. */
export const States = {
  render: () => (
    <div role="listbox" aria-label="Folders" style={grid}>
      <FolderTile
        name="Interviews"
        countLabel="12 items"
        active
        aria-label="Interviews, folder, 12 items"
      />
      <FolderTile name="B-roll" countLabel="1 item" selected aria-label="B-roll, folder, 1 item" />
      <FolderTile name="Music" countLabel="Empty" dropTarget aria-label="Music, folder, empty" />
      <FolderTile
        name="Day 1"
        countLabel="3 items"
        renaming
        renameLabel="Folder name"
        onRename={noop}
        onCancelRename={noop}
      />
    </div>
  ),
};

/** In search results across the whole bin the path sits under the name. */
export const WithPath = {
  render: () => (
    <div role="listbox" aria-label="Folders" style={grid}>
      <FolderTile
        name="Interviews"
        path="Media / Day 1"
        countLabel="4 items"
        aria-label="Interviews, folder, 4 items"
      />
    </div>
  ),
};
