// SPDX-License-Identifier: GPL-3.0-or-later
import { forwardRef, type HTMLAttributes } from "react";
import { Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";
import { InlineRename } from "./InlineRename.tsx";

export interface FolderTileProps
  extends Omit<HTMLAttributes<HTMLDivElement>, "role" | "children" | "onChange"> {
  /** The folder's name. */
  name: string;
  /** What it holds, already worded: "12 items". */
  countLabel: string;
  /** Part of the selection (`aria-selected`). */
  selected?: boolean | undefined;
  /** The one tile in the tab order (roving tabindex). */
  active?: boolean | undefined;
  /** Something is being dragged over it and may be dropped here. */
  dropTarget?: boolean | undefined;
  /** The name is being edited in place. */
  renaming?: boolean | undefined;
  /** Names the rename field: "Folder name". */
  renameLabel?: string | undefined;
  onRename?: ((name: string) => void) | undefined;
  onCancelRename?: (() => void) | undefined;
  /** Names a folder tile in the grid: "Interviews, folder, 3 items". */
  "aria-label"?: string | undefined;
  /** Path under the name, for search results across folders: "Interviews / Day 1". */
  path?: string | undefined;
}

/**
 * The folder variant of the bin's asset tile (design/components/AssetTile): a folder glyph where
 * the thumbnail would be, the name and the item count. An option of the same listbox as the
 * asset tiles; a drop target for dragged tiles. Layout (grid or list) follows the grid around it.
 */
export const FolderTile = forwardRef<HTMLDivElement, FolderTileProps>(function FolderTile(
  {
    name,
    countLabel,
    selected = false,
    active = false,
    dropTarget = false,
    renaming = false,
    renameLabel = "Folder name",
    onRename,
    onCancelRename,
    path,
    className,
    ...rest
  },
  ref,
) {
  return (
    <div
      ref={ref}
      role="option"
      aria-selected={selected}
      tabIndex={active ? 0 : -1}
      data-drop-target={dropTarget ? "true" : undefined}
      className={cx("cw-foldertile", className)}
      {...rest}
    >
      <span className="cw-foldertile__glyph" aria-hidden="true">
        <Icon name={dropTarget ? "folder-open" : "folder"} size="lg" />
      </span>
      <span className="cw-foldertile__body">
        {renaming ? (
          <InlineRename
            value={name}
            label={renameLabel}
            onCommit={(next) => onRename?.(next)}
            onCancel={() => onCancelRename?.()}
          />
        ) : (
          <span className="cw-foldertile__name body-strong">{name}</span>
        )}
        <span className="cw-foldertile__meta caption">
          {path === undefined ? null : <span className="cw-foldertile__path">{path}</span>}
          <span className="numeric">{countLabel}</span>
        </span>
      </span>
    </div>
  );
});
