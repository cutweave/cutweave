// SPDX-License-Identifier: GPL-3.0-or-later
// The bin's sidebar (WP-2F.06c, ADR-0036): the folder tree and the collections beside the grid.
// A folder is where a clip is; a collection is a list it is on. A click opens either in the bin;
// a folder row takes dropped cards and folders exactly as the breadcrumb does (one `media.move`),
// a manual collection row takes dropped cards (one `media.add_to_collection`). The one row that is
// open carries the marigold mark. Two trees with a roving tab stop each: arrows move, Right and
// Left open and close a folder, Enter opens, and on a collection F2 renames and Delete asks.
import type { CollectionId, FolderId, MediaId } from "@cutweave/protocol";
import { Icon, IconButton, InlineRename, Tooltip } from "@cutweave/ui";
import {
  type DragEvent,
  type KeyboardEvent,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useEngineReady } from "../engine/engineStore.ts";
import { useContextMenu } from "../menus/host.tsx";
import { useDocStore } from "../state/docStore.ts";
import { selectMediaCards, selectMediaUsage } from "./cards.ts";
import { renameCollection } from "./collectionActions.ts";
import {
  addMediaToCollection,
  newCollectionInPlace,
  requestCollectionDelete,
} from "./collectionCommands.ts";
import { type CollectionEntry, selectCollections } from "./collections.ts";
import { dragStore, tileDragStore } from "./dragStore.ts";
import { DROP_ATTRIBUTE } from "./folders.ts";
import { openFolder } from "./folderUi.ts";
import { binDeps } from "./requestDelete.ts";
import {
  canDropOnCollection,
  type FolderRow,
  folderRows,
  openCollection,
  type SidebarMode,
  sidebarUiStore,
  treeMove,
  useSidebarUi,
} from "./sidebar.ts";

/** The drag events of a drop target, as the bin hands them out for a folder. */
export interface SidebarDropHandlers {
  onDragEnter(e: DragEvent<HTMLElement>): void;
  onDragOver(e: DragEvent<HTMLElement>): void;
  onDragLeave(e: DragEvent<HTMLElement>): void;
  onDrop(e: DragEvent<HTMLElement>): void;
}

export interface BinSidebarProps {
  /** Beside the grid, or over it in a narrow panel. */
  readonly mode: SidebarMode;
  /** The folder the bin has open (`null`: the top level, unless a collection is open). */
  readonly open: FolderId | null;
  /** The collection the bin has open in place of a folder. */
  readonly collection: CollectionId | null;
  /** The folder a drag is over (the bin's; folder tiles and the breadcrumb show it too). */
  readonly dropOver: FolderId | null | undefined;
  /** What dropping on a folder does: the bin's own rule, shared with tiles and the breadcrumb. */
  dropOn(target: FolderId | null): SidebarDropHandlers;
  /** Something was opened or Escape was pressed: an overlay closes. */
  onDone(): void;
}

/** Moves the keyboard focus to row `index` of the tree `root`. */
function focusRow(root: HTMLElement | null, index: number): void {
  root?.querySelector<HTMLElement>(`[data-row="${index}"]`)?.focus();
}

interface CollectionRowProps {
  readonly entry: CollectionEntry;
  readonly index: number;
  readonly open: boolean;
  readonly tabStop: boolean;
  readonly renaming: boolean;
  readonly dropTarget: boolean;
  onOpen(entry: CollectionEntry): void;
  onFocus(index: number): void;
}

function CollectionRow({
  entry,
  index,
  open,
  tabStop,
  renaming,
  dropTarget,
  onOpen,
  onFocus,
}: CollectionRowProps) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const menuRef = useContextMenu<HTMLDivElement>("bin.collection", () => ({
    ids: [entry.id],
    name: entry.name,
  }));
  const ui = sidebarUiStore.getState();
  const kind = t(
    entry.kind === "smart" ? "mediaBin.collections.kindSmart" : "mediaBin.collections.kindManual",
  );
  const over = (e: DragEvent<HTMLElement>) => {
    const payload = tileDragStore.getState().payload;
    // Not preventing the default tells the browser this is not a place to drop.
    if (payload === null || !canDropOnCollection(project, payload, entry.id)) return;
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = "copy";
    ui.setDropOver(entry.id);
  };
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: the tree handles the keys (arrows, Enter, F2, Delete)
    <div
      ref={menuRef}
      role="treeitem"
      aria-level={1}
      aria-selected={open}
      aria-label={t(
        entry.count === 0
          ? "mediaBin.collections.rowLabelEmpty"
          : entry.count === 1
            ? "mediaBin.collections.rowLabelOne"
            : "mediaBin.collections.rowLabelMany",
        { name: entry.name, kind, count: entry.count },
      )}
      tabIndex={tabStop ? 0 : -1}
      data-row={index}
      data-collection-id={entry.id}
      data-collection-kind={entry.kind}
      data-drop-target={dropTarget ? "true" : undefined}
      className="cw-binside__row"
      onClick={() => onOpen(entry)}
      onFocus={() => onFocus(index)}
      onDragEnter={over}
      onDragOver={over}
      onDragLeave={(e) => {
        const next = e.relatedTarget as Node | null;
        if (next !== null && e.currentTarget.contains(next)) return;
        if (sidebarUiStore.getState().dropOver === entry.id) ui.setDropOver(null);
      }}
      onDrop={(e) => {
        const payload = tileDragStore.getState().payload;
        ui.setDropOver(null);
        if (payload === null || !canDropOnCollection(project, payload, entry.id)) return;
        e.preventDefault();
        e.stopPropagation();
        tileDragStore.getState().end();
        dragStore.getState().end();
        void addMediaToCollection(entry.id, payload.media as MediaId[]);
      }}
    >
      <Icon
        name={entry.kind === "smart" ? "search" : "layers"}
        size="md"
        className="cw-binside__icon"
      />
      {renaming ? (
        <InlineRename
          value={entry.name}
          label={t("mediaBin.collections.nameLabel")}
          onCommit={(name) => {
            ui.stopRename();
            void renameCollection(binDeps(), entry.id, entry.name, name);
          }}
          onCancel={() => ui.stopRename()}
        />
      ) : (
        <span className="cw-binside__name body" title={entry.name}>
          {entry.name}
        </span>
      )}
      <span className="cw-binside__count caption numeric" aria-hidden="true">
        {entry.count}
      </span>
    </div>
  );
}

/** The sidebar of the media bin: Folders, then Collections. */
export function BinSidebar({ mode, open, collection, dropOver, dropOn, onDone }: BinSidebarProps) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const ready = useEngineReady();
  const collapsed = useSidebarUi((s) => s.collapsed);
  const renaming = useSidebarUi((s) => s.renaming);
  const dropCollection = useSidebarUi((s) => s.dropOver);
  const rows = useMemo(() => folderRows(project, collapsed), [project, collapsed]);
  const collections = useMemo(
    () => selectCollections(project, selectMediaCards(project), selectMediaUsage(project)),
    [project],
  );
  const foldersId = useId();
  const collectionsId = useId();
  const folderTree = useRef<HTMLDivElement>(null);
  const collectionTree = useRef<HTMLDivElement>(null);

  // The tab stop of each tree: the open row when the tree has it, else the row last focused.
  const openFolderIndex = collection === null ? rows.findIndex((row) => row.id === open) : -1;
  const openCollectionIndex = collections.findIndex((entry) => entry.id === collection);
  const [folderFocus, setFolderFocus] = useState(0);
  // By ID: a rename moves a collection in the list, and the tab stop goes with it.
  const [collectionFocus, setCollectionFocus] = useState<CollectionId | null>(null);
  useEffect(() => {
    if (openFolderIndex >= 0) setFolderFocus(openFolderIndex);
  }, [openFolderIndex]);
  useEffect(() => {
    if (collection !== null) setCollectionFocus(collection);
  }, [collection]);
  const folderStop = Math.min(folderFocus, rows.length - 1);
  const collectionStop = Math.max(
    collections.findIndex((entry) => entry.id === collectionFocus),
    0,
  );
  const focusCollection = (index: number) => setCollectionFocus(collections[index]?.id ?? null);

  // An overlay takes the focus when it opens, so the keyboard is in it and Escape closes it.
  useEffect(() => {
    if (mode !== "overlay") return;
    const root = openCollectionIndex >= 0 ? collectionTree.current : folderTree.current;
    focusRow(root, openCollectionIndex >= 0 ? openCollectionIndex : Math.max(openFolderIndex, 0));
    // Only when the overlay opens: later changes of what is open must not move the focus.
  }, [mode]);

  // The collection that starts being renamed is on screen and keeps the tab stop.
  useEffect(() => {
    if (renaming === null) return;
    setCollectionFocus(renaming);
  }, [renaming]);

  // When a rename ends, the keyboard is back on the row whose name was edited.
  const wasRenaming = useRef(false);
  useEffect(() => {
    if (renaming !== null) wasRenaming.current = true;
    else if (wasRenaming.current) {
      wasRenaming.current = false;
      focusRow(collectionTree.current, collectionStop);
    }
  }, [renaming, collectionStop]);

  const openFolderRow = (row: FolderRow) => {
    openFolder(row.id);
    onDone();
  };
  const openCollectionRow = (entry: CollectionEntry) => {
    openCollection(entry.id);
    onDone();
  };

  const onFolderKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const row = rows[folderStop];
    if (row === undefined) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      openFolderRow(row);
      return;
    }
    if (e.key === "Escape" && mode === "overlay") {
      e.preventDefault();
      onDone();
      return;
    }
    const move = treeMove(rows, folderStop, e.key);
    if (move === null) return;
    e.preventDefault();
    e.stopPropagation();
    if (move.kind === "focus") {
      setFolderFocus(move.index);
      focusRow(folderTree.current, move.index);
    } else if (row.id !== null) {
      sidebarUiStore.getState().setCollapsed(row.id, move.kind === "collapse");
    }
  };

  const onCollectionKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const entry = collections[collectionStop];
    if (e.key === "Escape" && mode === "overlay") {
      e.preventDefault();
      onDone();
      return;
    }
    if (entry === undefined) return;
    switch (e.key) {
      case "Enter":
      case " ":
        e.preventDefault();
        openCollectionRow(entry);
        return;
      case "F2":
        e.preventDefault();
        e.stopPropagation();
        sidebarUiStore.getState().startRename(entry.id);
        return;
      case "Delete":
      case "Backspace":
        e.preventDefault();
        e.stopPropagation();
        void requestCollectionDelete(entry.id);
        return;
      default: {
        const move = treeMove(
          collections.map(() => ({ level: 1, hasChildren: false, expanded: false })),
          collectionStop,
          e.key,
        );
        if (move === null || move.kind !== "focus") return;
        e.preventDefault();
        e.stopPropagation();
        focusCollection(move.index);
        focusRow(collectionTree.current, move.index);
      }
    }
  };

  const newLabel = t("mediaBin.collections.newButton");
  return (
    <aside className="cw-binside" data-mode={mode} aria-label={t("mediaBin.sidebar.label")}>
      <div className="cw-binside__section">
        <div className="cw-binside__head">
          <span id={foldersId} className="cw-binside__heading label">
            {t("mediaBin.sidebar.folders")}
          </span>
        </div>
        <div ref={folderTree} role="tree" aria-labelledby={foldersId} onKeyDown={onFolderKey}>
          {rows.map((row, index) => {
            const isOpen = collection === null && row.id === open;
            const name = row.name ?? t("mediaBin.sidebar.allMedia");
            const drop = dropOn(row.id);
            return (
              // biome-ignore lint/a11y/useKeyWithClickEvents: the tree handles the keys (arrows, Enter)
              <div
                key={row.id ?? "top"}
                role="treeitem"
                aria-level={row.level}
                aria-selected={isOpen}
                aria-expanded={row.hasChildren ? row.expanded : undefined}
                aria-label={name}
                tabIndex={index === folderStop ? 0 : -1}
                data-row={index}
                data-folder-row={row.id ?? ""}
                data-drop-target={
                  dropOver !== undefined && dropOver === row.id ? "true" : undefined
                }
                {...{ [DROP_ATTRIBUTE]: row.id ?? "" }}
                className="cw-binside__row"
                style={{
                  paddingInlineStart: `calc(var(--space-1) + ${row.level - 1} * var(--space-3))`,
                }}
                onClick={() => openFolderRow(row)}
                onFocus={() => setFolderFocus(index)}
                {...drop}
              >
                {row.hasChildren && row.id !== null ? (
                  // A pointer shortcut; the keyboard opens and closes a folder with Right and Left.
                  <span
                    className="cw-binside__twist"
                    aria-hidden="true"
                    onClick={(e) => {
                      e.stopPropagation();
                      sidebarUiStore.getState().setCollapsed(row.id as FolderId, row.expanded);
                    }}
                  >
                    <Icon name={row.expanded ? "chevron-down" : "chevron-right"} size="sm" />
                  </span>
                ) : (
                  <span className="cw-binside__twist" aria-hidden="true" />
                )}
                <Icon
                  name={row.id === null ? "film" : isOpen ? "folder-open" : "folder"}
                  size="md"
                  className="cw-binside__icon"
                />
                <span className="cw-binside__name body" title={name}>
                  {name}
                </span>
              </div>
            );
          })}
        </div>
      </div>
      <div className="cw-binside__section">
        <div className="cw-binside__head">
          <span id={collectionsId} className="cw-binside__heading label">
            {t("mediaBin.sidebar.collections")}
          </span>
          <Tooltip label={newLabel}>
            <IconButton
              size="sm"
              icon="plus"
              label={newLabel}
              disabled={!ready}
              onClick={() => void newCollectionInPlace()}
            />
          </Tooltip>
        </div>
        {collections.length === 0 ? (
          <p className="cw-binside__hint caption">{t("mediaBin.collections.none")}</p>
        ) : (
          <div
            ref={collectionTree}
            role="tree"
            aria-labelledby={collectionsId}
            onKeyDown={onCollectionKey}
          >
            {collections.map((entry, index) => (
              <CollectionRow
                key={entry.id}
                entry={entry}
                index={index}
                open={entry.id === collection}
                tabStop={index === collectionStop}
                renaming={renaming === entry.id}
                dropTarget={dropCollection === entry.id}
                onOpen={openCollectionRow}
                onFocus={focusCollection}
              />
            ))}
          </div>
        )}
      </div>
    </aside>
  );
}
