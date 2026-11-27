// SPDX-License-Identifier: GPL-3.0-or-later
import type { FolderId, MediaId } from "@cutweave/protocol";
import type { DroppedMedia } from "@cutweave/timeline";
import {
  Button,
  FilterChip,
  FolderTile,
  Icon,
  IconButton,
  type IconName,
  InlineRename,
  MediaBreadcrumb,
  Menu,
  type MenuEntry,
  SearchField,
  SegmentedControl,
  StatusChip,
  Tooltip,
  useOwnerWindow,
} from "@cutweave/ui";
import {
  type DragEvent,
  type KeyboardEvent,
  type MouseEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { engineStore, useEngineStore } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { BinSidebar } from "../media/BinSidebar.tsx";
import { moveTiles, renameFolder, renameMedia } from "../media/binActions.ts";
import { newFolder, selectAllInBin } from "../media/binCommands.ts";
import {
  activeFilterCount,
  BIN_KINDS,
  type BinKind,
  binStore,
  DURATION_BUCKET_IDS,
  type DurationBucket,
  SORT_CHOICES,
  useBinStore,
} from "../media/binStore.ts";
import { computeBinView } from "../media/binView.ts";
import { formatDuration, type MediaCard, selectMediaUsage } from "../media/cards.ts";
import { openSaveSmartCollection } from "../media/collectionCommands.ts";
import { resolveOpenCollection } from "../media/collections.ts";
import { dragStore, MEDIA_DRAG_TYPE, TILE_DRAG_TYPE, tileDragStore } from "../media/dragStore.ts";
import {
  breadcrumbOf,
  canDrop,
  clickTile,
  type FolderTileData,
  folderPathText,
  resolveOpenFolder,
  type Tile,
  type TileSelection,
} from "../media/folders.ts";
import { countText } from "../media/folderText.ts";
import {
  currentOpenFolder,
  folderUiStore,
  openFolder,
  selectedTiles,
  useFolderUi,
} from "../media/folderUi.ts";
import { useIsOffline } from "../media/health.ts";
import { insertAtPlayhead } from "../media/insert.ts";
import { insertDurationStore } from "../media/insertDuration.ts";
import { isMovingPicture } from "../media/kinds.ts";
import { layoutStore } from "../media/layoutStore.ts";
import { useMediaStore } from "../media/mediaStore.ts";
import { type ProxyBadgeKind, useProxyBadge } from "../media/proxies.ts";
import { requestDeleteSelection } from "../media/requestDelete.ts";
import { sidebarMode, sidebarUiStore, useSidebarUi } from "../media/sidebar.ts";
import { projectTags, RATINGS, tagKey } from "../media/tags.ts";
import { useThumbnailSource } from "../media/thumbnails.ts";
import { useCardScrub } from "../media/useCardScrub.ts";
import { useImport } from "../media/useImport.ts";
import { ContextSurface, registerContextSurface, useContextMenu } from "../menus/host.tsx";
import { useSessionStore } from "../session/sessionStore.ts";
import { documentOf, forEveryWindow, observeSize } from "../shell/windows.ts";
import { docStore, useDocStore } from "../state/docStore.ts";
import { useJobStore } from "../state/jobStore.ts";
import { usePlaybackStore } from "../state/playbackStore.ts";
import { useSourceViewStore } from "../state/sourceViewStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { useViewStore, viewStore } from "../state/viewStore.ts";
import { translator } from "../timeline/errors.ts";
import { transportController } from "../transport/useTransport.ts";
import { binKeyAction } from "./binKeys.ts";
import { EmptyState } from "./EmptyState.tsx";

/** The modifier key cap for tooltips: ⌘ on macOS, Ctrl elsewhere. */
function modifierKey(): string {
  const platform = typeof navigator === "undefined" ? "" : navigator.platform;
  return /mac/i.test(platform) ? "⌘" : "Ctrl";
}

const KIND_ICON: Record<MediaCard["kind"], IconName> = {
  video: "film",
  audio: "audio-waveform",
  image: "image",
  image_sequence: "images",
  sequence_ref: "layers",
};

const KIND_KEY: Record<MediaCard["kind"], TKey> = {
  video: "mediaBin.kind.video",
  audio: "mediaBin.kind.audio",
  image: "mediaBin.kind.image",
  image_sequence: "mediaBin.kind.imageSequence",
  sequence_ref: "mediaBin.kind.sequenceRef",
};

const DURATION_KEY: Record<DurationBucket, TKey> = {
  under10s: "mediaBin.filters.under10s",
  "10sTo1m": "mediaBin.filters.10sTo1m",
  "1To5m": "mediaBin.filters.1To5m",
  over5m: "mediaBin.filters.over5m",
};

const USE_KEY: Record<"used" | "unused", TKey> = {
  used: "mediaBin.filters.used",
  unused: "mediaBin.filters.unused",
};

const SORT_KEY: Record<string, TKey> = {
  "imported-desc": "mediaBin.sort.importedDesc",
  "imported-asc": "mediaBin.sort.importedAsc",
  "name-asc": "mediaBin.sort.nameAsc",
  "name-desc": "mediaBin.sort.nameDesc",
  "duration-asc": "mediaBin.sort.durationAsc",
  "duration-desc": "mediaBin.sort.durationDesc",
  "kind-asc": "mediaBin.sort.kindAsc",
};

/** Tags a card in the list shows before "+N" (WP-2F.06b). */
const CARD_TAGS = 3;

/** A second click on the name of the only selected tile renames it after this long. */
const SLOW_CLICK_MS = 600;

/** Cards drawn beyond the visible rows, so scrolling never shows a gap. */
const OVERSCAN_ROWS = 2;

interface Metrics {
  /** Scroll offset and height of the list, CSS px. */
  top: number;
  height: number;
  /** Cards per row and the distance between row starts, CSS px (0 until measured). */
  cols: number;
  rowHeight: number;
}

/**
 * Which cards to draw. Until the list has a measured size (first paint, or no layout engine) every
 * card is drawn; after that only the rows in view plus a margin, with padding standing in for the
 * rest so the scroll bar keeps its length.
 */
function visibleRange(count: number, m: Metrics) {
  if (m.height === 0 || m.rowHeight === 0 || m.cols === 0) {
    return { start: 0, end: count, padTop: 0, padBottom: 0 };
  }
  const rows = Math.ceil(count / m.cols);
  const first = Math.max(0, Math.floor(m.top / m.rowHeight) - OVERSCAN_ROWS);
  const last = Math.min(rows - 1, Math.ceil((m.top + m.height) / m.rowHeight) + OVERSCAN_ROWS);
  return {
    start: first * m.cols,
    end: Math.min(count, (last + 1) * m.cols),
    padTop: first * m.rowHeight,
    padBottom: Math.max(0, (rows - 1 - last) * m.rowHeight),
  };
}

function isTextEntry(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (el === null || typeof el.tagName !== "string") return false;
  return el.isContentEditable || el.tagName === "INPUT" || el.tagName === "TEXTAREA";
}

/** What the timeline is told a card is: an image sequence is video there (ADR-0035). */
const dropKind = (kind: Exclude<MediaCard["kind"], "sequence_ref">): DroppedMedia["kind"] =>
  kind === "image_sequence" ? "video" : kind;

const dragOf = (card: MediaCard): DroppedMedia | null =>
  card.kind === "sequence_ref"
    ? null
    : {
        id: card.id,
        kind: dropKind(card.kind),
        // A still has no length of its own: its "Duration…" choice, else the drop's default.
        duration:
          card.kind === "image"
            ? (insertDurationStore.getState().byMedia.get(card.id) ?? null)
            : card.duration,
      };

/** What a drag of a bin tile is told about the tile it starts on. */
interface TileHandlers {
  onPress(tile: Tile, e: MouseEvent<HTMLElement>): void;
  onDragStart(tile: Tile, media: DroppedMedia | null, e: DragEvent<HTMLElement>): void;
  onDragEnd(): void;
  onCancelRename(): void;
}

interface DropHandlers {
  onDragEnter(e: DragEvent<HTMLElement>): void;
  onDragOver(e: DragEvent<HTMLElement>): void;
  onDragLeave(e: DragEvent<HTMLElement>): void;
  onDrop(e: DragEvent<HTMLElement>): void;
}

/** The proxy badge of a card (WP-2F.03b): its words, its tone and what the tooltip explains. */
const PROXY_LABEL = {
  ready: "mediaBin.proxy.ready",
  queued: "mediaBin.proxy.queued",
  failed: "mediaBin.proxy.failed.badge",
  evicted: "mediaBin.proxy.evicted",
  incomplete: "mediaBin.proxy.incomplete",
} as const satisfies Record<Exclude<ProxyBadgeKind, "generating">, string>;
const PROXY_TIP = {
  ready: "mediaBin.proxy.tip.ready",
  generating: "mediaBin.proxy.tip.generating",
  queued: "mediaBin.proxy.tip.queued",
  failed: "mediaBin.proxy.tip.failed",
  evicted: "mediaBin.proxy.tip.evicted",
  incomplete: "mediaBin.proxy.tip.incomplete",
} as const satisfies Record<ProxyBadgeKind, string>;
const PROXY_TONE = {
  ready: "success",
  generating: "neutral",
  queued: "neutral",
  failed: "warning",
  evicted: "neutral",
  incomplete: "neutral",
} as const satisfies Record<ProxyBadgeKind, "success" | "warning" | "neutral">;

interface CardViewProps extends TileHandlers {
  card: MediaCard;
  selected: boolean;
  /** The one card in the tab order (roving tabindex). */
  active: boolean;
  analyzing: boolean;
  failure: string | null;
  /** The folder path under the name: results across folders say where each one is. */
  path: string | null;
  renaming: boolean;
  /** How many clips on the timelines use it (0: unused). */
  uses: number;
  onSelect(id: MediaId): void;
  onInsert(card: MediaCard): void;
  onRename(card: MediaCard, name: string): void;
}

/** Whether a card can play on its own (ADR-0022): sound and video; stills have nothing to play. */
const playable = (card: MediaCard): boolean =>
  (isMovingPicture(card.kind) || card.kind === "audio") &&
  !card.broken &&
  card.duration !== null &&
  card.duration > 0;

/** Plays `card` in the Preview's Source view, or pauses it while it plays there. */
const playCard = (card: MediaCard): void =>
  void transportController().toggleSource(card.id, "preview");

function CardView({
  card,
  selected,
  active,
  analyzing,
  failure,
  path,
  renaming,
  uses,
  onSelect,
  onPress,
  onInsert,
  onDragStart,
  onDragEnd,
  onRename,
  onCancelRename,
}: CardViewProps) {
  const { t } = useTranslation();
  const source = useThumbnailSource();
  const indexed = useMediaStore((s) => s.indexed[card.id] ?? 0);
  const [failedImage, setFailedImage] = useState<string | null>(null);
  const thumb = source.thumbnailFor({ id: card.id, kind: card.kind, width: 160, indexed });
  const image = thumb.kind === "image" && thumb.src !== failedImage ? thumb.src : null;
  const drag = dragOf(card);
  // Right-click or the Menu key: the card's menu (WP-2H.13). The host selects the card first.
  const menuRef = useContextMenu<HTMLDivElement>("bin.card", () => ({
    ids: [card.id],
    name: card.name,
  }));
  const scrub = useCardScrub(card);
  const offline = useIsOffline(card.id);
  const proxy = useProxyBadge(card.id);
  const canPlay = playable(card);
  const playingHere = usePlaybackStore((s) => s.source?.media === card.id && s.playing);
  const shownInPreview = useSourceViewStore((s) => s.host === "preview");
  const playing = playingHere && shownInPreview;
  const playLabel = t(playing ? "mediaBin.card.pause" : "mediaBin.card.play", {
    name: card.name,
  });

  const size = card.width !== null && card.height !== null ? `${card.width}×${card.height}` : null;
  const meta = [size, card.fps === null ? null : t("mediaBin.card.fps", { fps: String(card.fps) })]
    .filter((part) => part !== null)
    .join(" · ");

  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: the listbox handles the keys (arrows, Enter, Space, Delete)
    <div
      ref={menuRef}
      role="option"
      aria-selected={selected}
      tabIndex={active ? 0 : -1}
      draggable={!renaming}
      data-media-id={card.id}
      data-kind={card.kind}
      className="cw-media__card"
      onClick={(e) => onPress({ kind: "media", id: card.id }, e)}
      onDoubleClick={() => onInsert(card)}
      onDragStart={(e) => {
        scrub.stop();
        onDragStart({ kind: "media", id: card.id }, drag, e);
      }}
      onDragEnd={onDragEnd}
    >
      <span
        className="cw-media__thumb"
        aria-hidden="true"
        ref={scrub.thumbRef}
        onPointerMove={scrub.onPointerMove}
        onPointerLeave={scrub.stop}
        onPointerCancel={scrub.stop}
      >
        {image === null ? (
          <Icon name={KIND_ICON[card.kind]} size="lg" className="cw-media__thumb-icon" />
        ) : (
          <img
            className="cw-media__thumb-img"
            src={image}
            alt=""
            draggable={false}
            onError={() => setFailedImage(image)}
          />
        )}
        {scrub.enabled ? (
          <>
            <canvas className="cw-media__scrub-canvas" ref={scrub.canvasRef} hidden />
            <span className="cw-media__scrub-line" ref={scrub.lineRef} />
            <span className="cw-media__scrub-time numeric" ref={scrub.timeRef} />
          </>
        ) : null}
        {canPlay ? (
          <span className="cw-media__play-slot" data-playing={playing}>
            <Tooltip label={playLabel}>
              {/* biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: a pointer shortcut in the card's picture (aria-hidden); a button would nest a control in the option, and the keyboard plays with Space on the card */}
              <span
                className="cw-iconbtn cw-iconbtn--sm cw-media__play"
                data-playing={playing}
                // It neither double-clicks into an insert nor scrubs the card.
                onClick={(e) => {
                  e.stopPropagation();
                  onSelect(card.id);
                  playCard(card);
                }}
                onDoubleClick={(e) => e.stopPropagation()}
                onPointerMove={(e) => e.stopPropagation()}
              >
                <Icon name={playing ? "pause" : "play"} size="md" />
              </span>
            </Tooltip>
          </span>
        ) : null}
        {card.duration !== null && card.kind !== "image" ? (
          <span className="cw-media__duration numeric">{formatDuration(card.duration)}</span>
        ) : null}
        {uses === 0 ? null : (
          // On a timeline (WP-2F.06c): the bin's "Used" filter, said on the card.
          <span
            className="cw-media__used caption"
            data-testid="used-badge"
            title={
              uses === 1
                ? t("mediaBin.card.usedTipOne")
                : t("mediaBin.card.usedTipMany", { count: uses })
            }
          >
            {t("mediaBin.card.used")}
          </span>
        )}
      </span>
      <span className="cw-media__body">
        <span className="cw-media__head">
          {renaming ? (
            <InlineRename
              value={card.name}
              label={t("mediaBin.folder.mediaNameLabel")}
              onCommit={(name) => onRename(card, name)}
              onCancel={onCancelRename}
            />
          ) : (
            <span className="cw-media__name body-strong">{card.name}</span>
          )}
          {card.rating === null && card.tags.length === 0 ? null : (
            // Tags and the rating (WP-2F.06b), after the name. The list shows the tags that fit
            // and the rating; the grid shows only a rating, over the picture's corner (CSS). Either
            // way a card keeps its height.
            <span className="cw-media__marks" data-testid="card-marks">
              {card.tags.length === 0 ? null : (
                <span
                  className="cw-media__tags"
                  title={t("mediaBin.tags.onCard", { tags: card.tags.join(", ") })}
                >
                  {card.tags.slice(0, CARD_TAGS).map((tag) => (
                    <span key={tag} className="cw-media__tag caption">
                      {tag}
                    </span>
                  ))}
                  {card.tags.length > CARD_TAGS ? (
                    <span className="cw-media__tag-more caption numeric">
                      {t("mediaBin.tags.more", { count: card.tags.length - CARD_TAGS })}
                    </span>
                  ) : null}
                </span>
              )}
              {card.rating === null ? null : (
                <span
                  className="cw-media__rating caption numeric"
                  data-rating={card.rating}
                  title={t("mediaBin.rating.onCard", { count: card.rating })}
                >
                  <Icon name="star" size="sm" />
                  {card.rating}
                </span>
              )}
            </span>
          )}
        </span>
        {path === null || path === "" ? null : (
          <span className="cw-media__path caption">{path}</span>
        )}
        <span className="cw-media__meta caption">
          <span>{t(KIND_KEY[card.kind])}</span>
          {meta === "" ? null : <span>{meta}</span>}
          {card.codec === null ? null : <span className="cw-media__badge">{card.codec}</span>}
        </span>
        {offline || card.broken || failure !== null || analyzing ? (
          <span className="cw-media__status" title={failure ?? undefined}>
            {offline ? (
              <StatusChip tone="danger">{t("mediaBin.status.offline")}</StatusChip>
            ) : card.broken ? (
              <StatusChip tone="danger">{t("mediaBin.status.broken")}</StatusChip>
            ) : failure !== null ? (
              <StatusChip tone="warning">{t("mediaBin.status.analysisFailed")}</StatusChip>
            ) : (
              <StatusChip tone="neutral">{t("mediaBin.status.analyzing")}</StatusChip>
            )}
          </span>
        ) : null}
        {proxy === null ? null : (
          <span
            className="cw-media__status"
            data-testid="proxy-badge"
            data-proxy={proxy.kind}
            title={
              proxy.kind === "failed" && proxy.detail !== null
                ? t("mediaBin.proxy.tip.failedBecause", { message: proxy.detail })
                : t(PROXY_TIP[proxy.kind])
            }
          >
            <StatusChip tone={PROXY_TONE[proxy.kind]}>
              {proxy.kind === "generating"
                ? t("mediaBin.proxy.generating", { percent: proxy.percent ?? 0 })
                : t(PROXY_LABEL[proxy.kind])}
            </StatusChip>
          </span>
        )}
      </span>
    </div>
  );
}

interface FolderViewProps extends TileHandlers {
  folder: FolderTileData;
  selected: boolean;
  active: boolean;
  renaming: boolean;
  dropTarget: boolean;
  /** Where the folder is, under its name (results across folders). */
  path: string | null;
  drop: DropHandlers;
  onOpen(folder: FolderTileData): void;
  onRename(folder: FolderTileData, name: string): void;
}

function FolderView({
  folder,
  selected,
  active,
  renaming,
  dropTarget,
  path,
  drop,
  onPress,
  onOpen,
  onDragStart,
  onDragEnd,
  onRename,
  onCancelRename,
}: FolderViewProps) {
  const { t } = useTranslation();
  const translate = translator(t);
  // Right-click or the Menu key: the folder's menu. The host selects the folder first.
  const menuRef = useContextMenu<HTMLDivElement>("bin.folder", () => ({
    ids: [folder.id],
    name: folder.name,
  }));
  return (
    <FolderTile
      ref={menuRef}
      name={folder.name}
      countLabel={countText(folder.items, translate)}
      kindLabel={t("mediaBin.folder.kind")}
      selected={selected}
      active={active}
      dropTarget={dropTarget}
      renaming={renaming}
      renameLabel={t("mediaBin.folder.nameLabel")}
      path={path ?? undefined}
      draggable={!renaming}
      data-folder-id={folder.id}
      data-drop-folder={folder.id}
      onClick={(e) => onPress({ kind: "folder", id: folder.id }, e)}
      onDoubleClick={() => onOpen(folder)}
      onDragStart={(e) => onDragStart({ kind: "folder", id: folder.id }, null, e)}
      onDragEnd={onDragEnd}
      onRename={(name) => onRename(folder, name)}
      onCancelRename={onCancelRename}
      {...drop}
    />
  );
}

/**
 * The media bin (WP-1M.03, folders WP-2F.08): the open folder's subfolders and assets as a grid or
 * list, imported from a native dialog (Ctrl+I) or by dropping files from the OS, dragged onto the
 * timeline or onto folders, or added at the playhead with Enter. Listbox semantics with a roving
 * tab stop: arrows move (and select), Enter inserts an asset or opens a folder, Delete removes.
 */
export function MediaPanel() {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const projectPath = useSessionStore((s) => s.path) ?? "";
  const text = useBinStore((s) => s.text);
  const filters = useBinStore((s) => s.filters);
  const sort = useBinStore((s) => s.sort);
  const scope = useBinStore((s) => s.scope);
  const openRaw = useBinStore((s) => s.openFolders[projectPath] ?? null);
  // A collection open in place of a folder (WP-2F.06c): its members, flat, from every folder.
  const collectionRaw = useBinStore((s) => s.openCollections[projectPath] ?? null);
  const collection = useMemo(
    () => resolveOpenCollection(project, collectionRaw),
    [project, collectionRaw],
  );
  const open = useMemo(
    () => (collection === null ? resolveOpenFolder(project, openRaw) : null),
    [project, openRaw, collection],
  );
  // The bin shows what the engine's `media.list` would list for the same filter, sort and folder
  // (the same predicate, run on the mirror: no round trip per keystroke).
  const view = useMemo(
    () => computeBinView(project, { text, filters, sort, scope }, open, collection),
    [project, text, filters, sort, scope, open, collection],
  );
  const usage = useMemo(() => selectMediaUsage(project), [project]);
  const openCollectionEntry = collection === null ? undefined : project?.collections?.[collection];
  const manualOpen = openCollectionEntry?.kind.type === "manual";
  const allCards = view.all;
  const cards = view.cards;
  const folders = view.folders;
  const order = view.order;
  const filterCount = activeFilterCount(filters);
  const searching = view.searching;
  const hasFolders = project !== null && Object.keys(project.folders ?? {}).length > 0;
  const selection = useViewStore((s) => s.selection);
  const selectedMedia = useMemo(
    () => selection.filter((r) => r.kind === "media").map((r) => r.id as MediaId),
    [selection],
  );
  const selectedId = (selectedMedia[0] ?? null) as MediaId | null;
  const selectedFolders = useFolderUi((s) => s.folders);
  const renaming = useFolderUi((s) => s.renaming);
  const dropOver = useFolderUi((s) => s.dropOver);
  const layout = useStore(layoutStore, (s) => s.layout);
  const ready = useEngineStore((s) => s.rpc !== null && s.connection === "ready");
  const failures = useMediaStore((s) => s.failures);
  // Only which media are being analyzed: an export's progress events must not re-render the bin
  // (WP-2G.04), so the selector returns a string that changes only when that set does.
  const analyzingKey = useJobStore((s) => {
    const ids = new Set<string>();
    for (const job of Object.values(s.jobs)) {
      if (
        job.kind === "analysis" &&
        job.media_id !== null &&
        (job.state === "queued" || job.state === "running")
      ) {
        ids.add(job.media_id);
      }
    }
    return [...ids].sort().join("\n");
  });
  const importer = useImport();
  const nFolders = folders.length;

  const analyzing = useMemo(
    () => new Set(analyzingKey === "" ? [] : analyzingKey.split("\n")),
    [analyzingKey],
  );

  // ---- focus and windowing ---------------------------------------------------------------
  const scroller = useRef<HTMLDivElement>(null);
  const grid = useRef<HTMLDivElement>(null);
  const [metrics, setMetrics] = useState<Metrics>({ top: 0, height: 0, cols: 0, rowHeight: 0 });
  // The tab stop, as a position in `order` (folders, then assets).
  const [focusIndex, setFocusIndex] = useState(0);
  const focusRequest = useRef(false);

  const selectedIndex = order.findIndex((tile) => tile.kind === "media" && tile.id === selectedId);
  // The tab stop follows focus; a selection made elsewhere (the inspector, another click) moves it.
  useEffect(() => {
    if (selectedIndex >= 0) setFocusIndex(selectedIndex);
  }, [selectedIndex]);
  const activeIndex = Math.min(Math.max(focusIndex, 0), Math.max(order.length - 1, 0));
  const activeTile = order[activeIndex];

  // Another folder or collection: back to the first tile and the top of the list.
  useEffect(() => {
    setFocusIndex(0);
    if (scroller.current !== null) scroller.current.scrollTop = 0;
  }, [open, collection]);

  // What is gone from the grid (deleted, moved away) is no longer selected.
  useEffect(() => {
    const ui = folderUiStore.getState();
    const known = new Set(order.filter((tile) => tile.kind === "folder").map((tile) => tile.id));
    const kept = ui.folders.filter((id) => known.has(id) || project?.folders?.[id] !== undefined);
    if (kept.length !== ui.folders.length) ui.setFolders(kept);
  }, [order, project]);

  /** Where the asset grid starts inside the scroller (folder tiles sit above it). */
  const gridOffset = useCallback((): number => {
    const el = scroller.current;
    const g = grid.current;
    if (el === null || g === null) return 0;
    return g.getBoundingClientRect().top - el.getBoundingClientRect().top + el.scrollTop;
  }, []);

  const measure = useCallback(() => {
    const el = scroller.current;
    const g = grid.current;
    if (el === null || g === null) return;
    const style = getComputedStyle(g);
    const cols = Math.max(
      1,
      style.gridTemplateColumns.split(" ").filter((c) => /\d/.test(c)).length,
    );
    const first = g.firstElementChild as HTMLElement | null;
    const rowGap = Number.parseFloat(style.rowGap) || 0;
    const rowHeight = first === null ? 0 : first.offsetHeight + rowGap;
    // The scroll position as the asset grid sees it: how far its top is above the viewport top.
    const top = el.getBoundingClientRect().top - g.getBoundingClientRect().top;
    setMetrics((m) =>
      m.top === top && m.height === el.clientHeight && m.cols === cols && m.rowHeight === rowHeight
        ? m
        : { top, height: el.clientHeight, cols, rowHeight },
    );
  }, []);

  // Measure after every layout-affecting change; follow the scroller's own size too.
  useLayoutEffect(measure, [measure, cards, folders, layout]);
  // Observed from the panel's own window, again when it moves to another (a detached window).
  const ownerWindow = useOwnerWindow();
  useEffect(() => {
    const el = scroller.current;
    if (el === null || ownerWindow === null) return undefined;
    return observeSize(el, measure);
  }, [measure, ownerWindow]);

  const win = visibleRange(cards.length, layout === "list" ? { ...metrics, cols: 1 } : metrics);

  // Give the keyboard focus to the tile a key press moved to, once it is drawn.
  useEffect(() => {
    if (!focusRequest.current) return;
    focusRequest.current = false;
    const tile = order[activeIndex];
    if (tile === undefined) return;
    const attribute = tile.kind === "folder" ? "data-folder-id" : "data-media-id";
    scroller.current?.querySelector<HTMLElement>(`[${attribute}="${tile.id}"]`)?.focus();
  });

  // ---- selection -------------------------------------------------------------------------
  const currentSelection = (): TileSelection => ({
    tiles: [
      ...folderUiStore.getState().folders.map((id): Tile => ({ kind: "folder", id })),
      ...viewStore
        .getState()
        .selection.filter((r) => r.kind === "media")
        .map((r): Tile => ({ kind: "media", id: r.id })),
    ],
    anchor: folderUiStore.getState().anchor,
  });

  const applySelection = useCallback((next: TileSelection) => {
    const ui = folderUiStore.getState();
    ui.setFolders(
      next.tiles.filter((tile) => tile.kind === "folder").map((tile) => tile.id as FolderId),
      next.anchor,
    );
    viewStore
      .getState()
      .setSelection(
        next.tiles
          .filter((tile) => tile.kind === "media")
          .map((tile) => ({ kind: "media" as const, id: tile.id as MediaId })),
      );
  }, []);

  const select = useCallback(
    (id: MediaId) =>
      applySelection({ tiles: [{ kind: "media", id }], anchor: { kind: "media", id } }),
    [applySelection],
  );

  const goTo = (index: number, extend = false) => {
    const tile = order[index];
    const el = scroller.current;
    if (tile === undefined || el === null) return;
    setFocusIndex(index);
    applySelection(clickTile(order, currentSelection(), tile, { ctrl: false, shift: extend }));
    focusRequest.current = true;
    const cardIndex = index - nFolders;
    if (cardIndex >= 0 && metrics.rowHeight > 0 && metrics.cols > 0) {
      const cols = layout === "list" ? 1 : metrics.cols;
      const rowTop = gridOffset() + Math.floor(cardIndex / cols) * metrics.rowHeight;
      if (rowTop < el.scrollTop) el.scrollTop = rowTop;
      else if (rowTop + metrics.rowHeight > el.scrollTop + el.clientHeight) {
        el.scrollTop = rowTop + metrics.rowHeight - el.clientHeight;
      }
    }
  };

  // A slow second click on the name of the only selected tile renames it, as in a file manager.
  const slowRename = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (slowRename.current !== null) clearTimeout(slowRename.current);
    },
    [],
  );
  const press = (tile: Tile, e: MouseEvent<HTMLElement>) => {
    const before = currentSelection();
    const sole =
      before.tiles.length === 1 &&
      before.tiles[0]?.kind === tile.kind &&
      before.tiles[0]?.id === tile.id;
    const ctrl = e.ctrlKey || e.metaKey;
    const onName =
      (e.target as HTMLElement).closest(".cw-media__name, .cw-foldertile__name") !== null;
    applySelection(clickTile(order, before, tile, { ctrl, shift: e.shiftKey }));
    const at = order.findIndex((o) => o.kind === tile.kind && o.id === tile.id);
    if (at >= 0) setFocusIndex(at);
    if (slowRename.current !== null) clearTimeout(slowRename.current);
    slowRename.current = null;
    if (sole && onName && !ctrl && !e.shiftKey) {
      slowRename.current = setTimeout(() => {
        slowRename.current = null;
        folderUiStore.getState().startRename(tile);
      }, SLOW_CLICK_MS);
    }
  };

  // ---- actions ---------------------------------------------------------------------------
  const tr = useRef(translator(t));
  tr.current = translator(t);
  const actionDeps = useMemo(
    () => ({
      doc: docStore,
      view: viewStore,
      engine: engineStore,
      toasts: toastStore,
      t: (
        key: Parameters<ReturnType<typeof translator>>[0],
        vars?: Readonly<Record<string, string>>,
      ) => tr.current(key, vars),
    }),
    [],
  );
  const binDeps = useMemo(
    () => ({ engine: engineStore, toasts: toastStore, t: actionDeps.t }),
    [actionDeps],
  );

  const insert = useCallback(
    (card: MediaCard) => {
      const drag = dragOf(card);
      if (drag !== null) void insertAtPlayhead(actionDeps, drag);
    },
    [actionDeps],
  );

  const openFolderTile = useCallback((folder: { id: FolderId }) => openFolder(folder.id), []);

  const goUp = () => {
    // Out of a collection: back to the folders.
    if (collection !== null) openFolder(null);
    if (open === null) return;
    openFolder(project?.folders?.[open]?.parent ?? null);
  };

  // Delete: the selected assets and folders (a folder that holds something asks first).
  const requestDelete = () => {
    const sel = selectedTiles();
    if (sel.media.length === 0 && sel.folders.length === 0) {
      if (activeTile === undefined) return;
      applySelection({ tiles: [activeTile], anchor: activeTile });
    }
    void requestDeleteSelection();
  };
  const onlyFoldersSelected = selectedMedia.length === 0 && selectedFolders.length > 0;

  const finishRename = () => folderUiStore.getState().stopRename();
  const renameCard = (card: MediaCard, name: string) => {
    finishRename();
    void renameMedia(binDeps, card.id, card.name, name);
  };
  const renameFolderTile = (folder: FolderTileData, name: string) => {
    finishRename();
    void renameFolder(binDeps, folder.id, folder.name, name);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    // The keys are the table in binKeys.ts (Settings > Keyboard shortcuts lists the same one).
    const cols = layout === "list" ? 1 : Math.max(metrics.cols, 1);
    const last = order.length - 1;
    let next: number | null = null;
    // Folders sit in their own grid above the assets: a column move from one to the other lands on
    // the nearest tile, so the arrows treat the two grids as one row-major list.
    switch (binKeyAction(e)) {
      case "moveNext":
        next = layout === "list" ? null : activeIndex + 1;
        break;
      case "movePrevious":
        next = layout === "list" ? null : activeIndex - 1;
        break;
      case "moveDown":
        next = activeIndex + cols;
        break;
      case "moveUp":
        next = activeIndex - cols;
        break;
      case "first":
        next = 0;
        break;
      case "last":
        next = last;
        break;
      case "insert": {
        if (activeTile?.kind === "folder") openFolder(activeTile.id as FolderId);
        else {
          const card = cards.find((c) => c.id === activeTile?.id);
          if (card !== undefined) insert(card);
        }
        e.preventDefault();
        return;
      }
      case "play": {
        // Space selects the focused tile, as before, then plays or pauses an asset in the Source view.
        if (activeTile !== undefined) {
          applySelection({ tiles: [activeTile], anchor: activeTile });
          const card =
            activeTile.kind === "media" ? cards.find((c) => c.id === activeTile.id) : undefined;
          if (card !== undefined && playable(card)) playCard(card);
        }
        e.preventDefault();
        return;
      }
      case "remove":
        requestDelete();
        e.preventDefault();
        return;
      case "up":
        goUp();
        e.preventDefault();
        return;
      case "rename":
        if (activeTile !== undefined) folderUiStore.getState().startRename(activeTile);
        e.preventDefault();
        return;
      default:
        return;
    }
    e.preventDefault();
    if (next !== null) goTo(Math.min(Math.max(next, 0), last), e.shiftKey);
  };

  // Ctrl/Cmd+I is announced by the empty state: it opens the file dialog from anywhere in the app.
  const pickRef = useRef(importer.pickAndImport);
  pickRef.current = importer.pickAndImport;
  const pickFoldersRef = useRef(importer.pickFoldersAndImport);
  pickFoldersRef.current = importer.pickFoldersAndImport;
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      const action = binKeyAction(e);
      if (action !== "import" && action !== "importFolder") return;
      if (e.defaultPrevented || isTextEntry(e.target)) return;
      e.preventDefault();
      const pick = action === "import" ? pickRef.current : pickFoldersRef.current;
      void pick(currentOpenFolder());
    };
    // In every editor window, a detached panel's too.
    return forEveryWindow((win) => {
      win.addEventListener("keydown", onKey);
      return () => win.removeEventListener("keydown", onKey);
    });
  }, []);

  // Ctrl/Cmd+Shift+N and Ctrl/Cmd+A work anywhere in the panel except in a text field.
  const onPanelKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (isTextEntry(e.target)) return;
    const action = binKeyAction(e);
    if (action === "newFolder") {
      e.preventDefault();
      if (ready) void newFolder(open);
    } else if (action === "selectAll") {
      e.preventDefault();
      selectAllInBin();
    }
  };

  // ---- OS file drop and tile drops -------------------------------------------------------
  const [fileOver, setFileOver] = useState(false);
  const hasFiles = (e: DragEvent<HTMLElement>) => e.dataTransfer.types.includes("Files");
  const onDragOver = (e: DragEvent<HTMLElement>) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = ready ? "copy" : "none";
    setFileOver(true);
  };
  const onDragLeave = (e: DragEvent<HTMLElement>) => {
    const next = e.relatedTarget as Node | null;
    if (next === null || !e.currentTarget.contains(next)) setFileOver(false);
  };
  /** Files dropped from the OS go to `folder` (the open one, or the folder tile they landed on). */
  const importFiles = (e: DragEvent<HTMLElement>, folder: FolderId | null) => {
    const paths = importer.pathsOf(Array.from(e.dataTransfer.files));
    if (paths === null) {
      toastStore
        .getState()
        .push({ tone: "info", title: tr.current("mediaBin.import.desktopOnly") });
      return;
    }
    void importer.importPaths(paths, folder);
  };
  const onDrop = (e: DragEvent<HTMLElement>) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    setFileOver(false);
    importFiles(e, open);
  };

  const dropOn = (target: FolderId | null): DropHandlers => {
    const over = (e: DragEvent<HTMLElement>) => {
      const payload = tileDragStore.getState().payload;
      if (payload !== null) {
        // Not preventing the default tells the browser this is not a place to drop.
        if (!canDrop(project, payload, target)) return;
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = "move";
        folderUiStore.getState().setDropOver(target);
      } else if (hasFiles(e) && ready) {
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = "copy";
        folderUiStore.getState().setDropOver(target);
      }
    };
    return {
      onDragEnter: over,
      onDragOver: over,
      onDragLeave: (e) => {
        const next = e.relatedTarget as Node | null;
        if (next !== null && e.currentTarget.contains(next)) return;
        if (folderUiStore.getState().dropOver === target) {
          folderUiStore.getState().setDropOver(undefined);
        }
      },
      onDrop: (e) => {
        const payload = tileDragStore.getState().payload;
        folderUiStore.getState().setDropOver(undefined);
        if (payload !== null) {
          if (!canDrop(project, payload, target)) return;
          e.preventDefault();
          e.stopPropagation();
          tileDragStore.getState().end();
          dragStore.getState().end();
          void moveTiles(
            binDeps,
            { media: payload.media as MediaId[], folders: payload.folders as FolderId[] },
            target,
          );
        } else if (hasFiles(e)) {
          e.preventDefault();
          e.stopPropagation();
          setFileOver(false);
          importFiles(e, target);
        }
      },
    };
  };

  const onTileDragStart = (tile: Tile, media: DroppedMedia | null, e: DragEvent<HTMLElement>) => {
    const sel = currentSelection();
    const part = sel.tiles.some((s) => s.kind === tile.kind && s.id === tile.id);
    if (!part) applySelection({ tiles: [tile], anchor: tile });
    const dragged = part ? sel.tiles : [tile];
    tileDragStore.getState().begin({
      media: dragged.filter((d) => d.kind === "media").map((d) => d.id),
      folders: dragged.filter((d) => d.kind === "folder").map((d) => d.id),
    });
    e.dataTransfer.setData(TILE_DRAG_TYPE, "1");
    if (media === null) {
      e.dataTransfer.effectAllowed = "move";
      return;
    }
    // An asset can also go onto the timeline (a copy), so the drag allows both.
    e.dataTransfer.setData(MEDIA_DRAG_TYPE, media.id);
    e.dataTransfer.effectAllowed = "copyMove";
    dragStore.getState().begin(media);
  };
  const onTileDragEnd = () => {
    tileDragStore.getState().end();
    dragStore.getState().end();
    folderUiStore.getState().setDropOver(undefined);
  };

  const handlers: TileHandlers = {
    onPress: press,
    onDragStart: onTileDragStart,
    onDragEnd: onTileDragEnd,
    onCancelRename: finishRename,
  };

  // ---- search, filters, sort ---------------------------------------------------------------
  const searchRef = useRef<HTMLInputElement>(null);
  const sectionRef = useRef<HTMLElement>(null);

  // ---- the sidebar (WP-2F.06c) -------------------------------------------------------------
  // Beside the grid when the panel is wide enough; in a narrow panel the toggle opens it over the
  // grid and it closes once something is opened. Shown or not is the panel's own, kept choice.
  const sidebarWanted = useBinStore((s) => s.sidebar);
  const sidebarOverlay = useSidebarUi((s) => s.overlay);
  const [panelWidth, setPanelWidth] = useState(0);
  useEffect(() => {
    const el = sectionRef.current;
    if (el === null || ownerWindow === null) return undefined;
    const read = () => setPanelWidth(el.clientWidth);
    read();
    return observeSize(el, read);
  }, [ownerWindow]);
  const sidebarLayout = sidebarMode(panelWidth);
  const sidebarShown = sidebarLayout === "inline" ? sidebarWanted : sidebarOverlay;
  useEffect(() => {
    if (sidebarLayout === "inline") sidebarUiStore.getState().setOverlay(false);
  }, [sidebarLayout]);
  const sidebarToggle = useRef<HTMLButtonElement>(null);
  const toggleSidebar = () => {
    if (sidebarLayout === "inline") binStore.getState().setSidebar(!sidebarWanted);
    else sidebarUiStore.getState().setOverlay(!sidebarOverlay);
  };
  const closeOverlay = (how: "opened" | "dismissed") => {
    if (sidebarLayout !== "overlay" || !sidebarUiStore.getState().overlay) return;
    sidebarUiStore.getState().setOverlay(false);
    // Escape goes back to the button that opened it; opening a row goes on into the grid.
    if (how === "dismissed") sidebarToggle.current?.focus();
    else focusRequest.current = true;
  };

  // Anywhere in the panel that is not a tile: Import, New folder, Sort by, View as (WP-2H.13). A
  // right-click on the empty bin clears the selection, as the Explorer does.
  useEffect(() => {
    const section = sectionRef.current;
    if (section === null) return undefined;
    return registerContextSurface(section, () => ({ surface: "bin.background" }), {
      name: "bin.background",
    });
  }, []);

  // A card selected from elsewhere (the timeline's "Reveal in media bin") scrolls into view.
  useEffect(() => {
    const el = scroller.current;
    if (selectedId === null || el === null || metrics.rowHeight === 0 || metrics.cols === 0) return;
    const index = cards.findIndex((c) => c.id === selectedId);
    if (index < 0) return;
    const cols = layout === "list" ? 1 : metrics.cols;
    const rowTop = gridOffset() + Math.floor(index / cols) * metrics.rowHeight;
    if (rowTop < el.scrollTop) el.scrollTop = rowTop;
    else if (rowTop + metrics.rowHeight > el.scrollTop + el.clientHeight) {
      el.scrollTop = rowTop + metrics.rowHeight - el.clientHeight;
    }
    // Only a change of the selection scrolls; a scroll the person made stays.
  }, [selectedId]);

  // Ctrl/Cmd+F focuses the search field while the Assets panel has focus.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (binKeyAction(e) !== "search") return;
      const root = sectionRef.current?.closest(".cw-assets") ?? sectionRef.current;
      // The focus of the panel's own window (a detached panel's window has its own).
      if (e.defaultPrevented || root === null || !root.contains(documentOf(root).activeElement)) {
        return;
      }
      e.preventDefault();
      searchRef.current?.focus();
      searchRef.current?.select();
    };
    return forEveryWindow((win) => {
      win.addEventListener("keydown", onKey);
      return () => win.removeEventListener("keydown", onKey);
    });
  }, []);

  const bin = binStore.getState();
  const durationLabel = (id: DurationBucket) => t(DURATION_KEY[id]);
  const kindLabel = (kind: BinKind) => t(KIND_KEY[kind]);
  const useLabel = (use: "used" | "unused") => t(USE_KEY[use]);

  // Tags and Rating (WP-2F.06b): the project's own tags (an asset must have all the checked
  // ones) and a least number of stars, each in a submenu so the menu stays short.
  const knownTags = useMemo(() => projectTags(allCards), [allCards]);
  const ratingLabel = (stars: number) =>
    stars === 5
      ? t("mediaBin.filters.ratingFive")
      : stars === 1
        ? t("mediaBin.filters.ratingFromOne")
        : t("mediaBin.filters.ratingFrom", { count: stars });
  const tagOn = (tag: string) => filters.tags.some((have) => tagKey(have) === tagKey(tag));

  const filterItems: MenuEntry[] = [
    { kind: "label", id: "kind-h", label: t("mediaBin.filters.kindHeading") },
    ...BIN_KINDS.map(
      (kind): MenuEntry => ({
        id: `kind-${kind}`,
        label: kindLabel(kind),
        checked: filters.kinds.includes(kind),
        keepOpen: true,
        onSelect: () => bin.toggleKind(kind),
      }),
    ),
    { kind: "separator", id: "sep-duration" },
    { kind: "label", id: "duration-h", label: t("mediaBin.filters.durationHeading") },
    {
      id: "duration-any",
      label: t("mediaBin.filters.durationAny"),
      checked: filters.duration === null,
      choice: "radio",
      keepOpen: true,
      onSelect: () => bin.setDuration(null),
    },
    ...DURATION_BUCKET_IDS.map(
      (id): MenuEntry => ({
        id: `duration-${id}`,
        label: durationLabel(id),
        checked: filters.duration === id,
        choice: "radio",
        keepOpen: true,
        onSelect: () => bin.setDuration(id),
      }),
    ),
    { kind: "separator", id: "sep-use" },
    { kind: "label", id: "use-h", label: t("mediaBin.filters.useHeading") },
    {
      id: "use-any",
      label: t("mediaBin.filters.useAny"),
      checked: filters.use === null,
      choice: "radio",
      keepOpen: true,
      onSelect: () => bin.setUse(null),
    },
    ...(["used", "unused"] as const).map(
      (use): MenuEntry => ({
        id: `use-${use}`,
        label: useLabel(use),
        checked: filters.use === use,
        choice: "radio",
        keepOpen: true,
        onSelect: () => bin.setUse(use),
      }),
    ),
    { kind: "separator", id: "sep-problems" },
    {
      id: "problems",
      label: t("mediaBin.filters.problems"),
      checked: filters.problems,
      keepOpen: true,
      onSelect: () => bin.setProblems(!filters.problems),
    },
    { kind: "separator", id: "sep-organize" },
    {
      kind: "submenu",
      id: "tags",
      label: t("mediaBin.filters.tagsHeading"),
      disabled: knownTags.length === 0,
      disabledReason: knownTags.length === 0 ? t("mediaBin.filters.noTags") : undefined,
      items: knownTags.map((entry) => ({
        id: `tag-${entry.tag}`,
        label: entry.tag,
        checked: tagOn(entry.tag),
        keepOpen: true,
        onSelect: () => bin.toggleTag(entry.tag),
      })),
    },
    {
      kind: "submenu",
      id: "rating",
      label: t("mediaBin.filters.ratingHeading"),
      items: [
        {
          id: "rating-any",
          label: t("mediaBin.filters.ratingAny"),
          checked: filters.minRating === null,
          choice: "radio",
          keepOpen: true,
          onSelect: () => bin.setMinRating(null),
        },
        ...RATINGS.map((stars) => ({
          id: `rating-${stars}`,
          label: ratingLabel(stars),
          checked: filters.minRating === stars,
          choice: "radio" as const,
          keepOpen: true,
          onSelect: () => bin.setMinRating(stars),
        })),
      ],
    },
  ];

  const sortItems: MenuEntry[] = [
    { kind: "label", id: "sort-h", label: t("mediaBin.sort.heading") },
    ...SORT_CHOICES.map(
      (choice): MenuEntry => ({
        id: `sort-${choice.by}-${choice.order}`,
        label: t(SORT_KEY[`${choice.by}-${choice.order}`] as TKey),
        checked: sort.by === choice.by && sort.order === choice.order,
        choice: "radio",
        onSelect: () => bin.setSort(choice),
      }),
    ),
  ];

  // One chip per active filter, each removable; the search text has its own clear button.
  const chips: { id: string; label: string; remove: () => void }[] = [
    ...filters.kinds.map((kind) => ({
      id: `kind-${kind}`,
      label: kindLabel(kind),
      remove: () => bin.toggleKind(kind),
    })),
    ...(filters.duration === null
      ? []
      : [
          {
            id: "duration",
            label: durationLabel(filters.duration),
            remove: () => bin.setDuration(null),
          },
        ]),
    ...(filters.use === null
      ? []
      : [{ id: "use", label: useLabel(filters.use), remove: () => bin.setUse(null) }]),
    ...(filters.problems
      ? [
          {
            id: "problems",
            label: t("mediaBin.filters.problems"),
            remove: () => bin.setProblems(false),
          },
        ]
      : []),
    ...filters.tags.map((tag) => ({
      id: `tag-${tag}`,
      label: t("mediaBin.filters.tagChip", { tag }),
      remove: () => bin.toggleTag(tag),
    })),
    ...(filters.minRating === null
      ? []
      : [
          {
            id: "rating",
            label: ratingLabel(filters.minRating),
            remove: () => bin.setMinRating(null),
          },
        ]),
  ];

  const importLabel = t("mediaBin.importButton");
  const importFolderLabel = t("mediaBin.importFolderButton");
  const newFolderLabel = t("mediaBin.folder.newButton");
  const filtersLabel =
    filterCount > 0
      ? t("mediaBin.filters.buttonActive", { count: filterCount })
      : t("mediaBin.filters.button");
  const sortLabel = t("mediaBin.sort.button");
  const deleteLabel = onlyFoldersSelected
    ? t("mediaBin.folder.deleteButton")
    : t("mediaBin.remove.button");
  const nothingSelected = selectedMedia.length === 0 && selectedFolders.length === 0;
  const crumbs = breadcrumbOf(project, open);
  const segments: { id: string | null; label: string }[] = crumbs.map((crumb) => ({
    id: crumb.id,
    label: crumb.name ?? t("mediaBin.folder.topLevel"),
  }));
  // An open collection is the place: "Media > Selects". Nothing drops on it here (the sidebar
  // row takes cards); the first segment leads back to the folders.
  if (collection !== null && openCollectionEntry !== undefined) {
    segments.push({ id: collection, label: openCollectionEntry.name });
  }
  const crumbDropFor = (id: string | null) => dropOn(id as FolderId | null);
  const showBreadcrumb = hasFolders || open !== null || collection !== null;
  const nothingAtAll = allCards.length === 0 && !hasFolders && collection === null;
  const emptyCollection = collection !== null && !searching && cards.length === 0;
  const emptyFolder =
    collection === null &&
    !searching &&
    folders.length === 0 &&
    cards.length === 0 &&
    !nothingAtAll;
  // Results across folders, and a collection's members, say where each one is.
  const pathOf = (folder: FolderId | null): string | null =>
    (searching || collection !== null) && folder !== null ? folderPathText(project, folder) : null;
  // "Save as smart collection" (WP-2F.06c): while a search or a filter is on, outside a collection.
  const canSaveSmart = searching && collection === null && ready;
  const sidebarLabel = t(sidebarShown ? "mediaBin.sidebar.hide" : "mediaBin.sidebar.show");
  const sortFixedLabel = t("mediaBin.collections.sortFixed");

  return (
    <section
      ref={sectionRef}
      className="cw-media"
      aria-label={t("mediaBin.panelLabel")}
      data-drop-active={fileOver ? "true" : undefined}
      onKeyDown={onPanelKeyDown}
      onDoubleClick={(e) => {
        // A double-click on the empty body of the panel imports, as the Import button does.
        const target = e.target as HTMLElement;
        const inBody = target.closest(".cw-media__scroll") !== null;
        const onControl = target.closest("[role=option], button, input, a") !== null;
        if (inBody && !onControl && ready && !importer.busy) {
          void importer.pickAndImport(open);
        }
      }}
      onDragOver={onDragOver}
      onDragEnter={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      {allCards.length > 0 || hasFolders ? (
        <div className="cw-media__search">
          <SearchField
            ref={searchRef}
            value={text}
            onChange={(next) => bin.setText(next)}
            label={t("mediaBin.search.label")}
            clearLabel={t("mediaBin.search.clear")}
            onArrowDown={() => goTo(0)}
          />
          {hasFolders ? (
            <SegmentedControl
              className="cw-media__scope"
              aria-label={t("mediaBin.folder.scopeLabel")}
              value={scope}
              options={[
                { value: "all", label: t("mediaBin.folder.scopeAll") },
                { value: "folder", label: t("mediaBin.folder.scopeFolder") },
              ]}
              onChange={(next) => bin.setScope(next)}
            />
          ) : null}
        </div>
      ) : null}
      <div className="cw-media__tools">
        {/* Icons with tooltips: the panel is narrow and its toolbar is for doing, not reading. */}
        <Tooltip label={sidebarLabel}>
          <IconButton
            ref={sidebarToggle}
            size="sm"
            icon="panel-left"
            label={sidebarLabel}
            pressed={sidebarShown}
            data-testid="bin-sidebar-toggle"
            onClick={toggleSidebar}
          />
        </Tooltip>
        <Tooltip label={importLabel} keys={[modifierKey(), "I"]}>
          <IconButton
            size="sm"
            icon="upload"
            label={importLabel}
            disabled={!ready || importer.busy}
            onClick={() => void importer.pickAndImport(open)}
          />
        </Tooltip>
        <Tooltip label={importFolderLabel} keys={[modifierKey(), "Alt", "I"]}>
          <IconButton
            size="sm"
            icon="folder-open"
            label={importFolderLabel}
            disabled={!ready || importer.busy}
            onClick={() => void importer.pickFoldersAndImport(open)}
          />
        </Tooltip>
        <Tooltip label={newFolderLabel} keys={[modifierKey(), "Shift", "N"]}>
          <IconButton
            size="sm"
            icon="folder-plus"
            label={newFolderLabel}
            disabled={!ready}
            onClick={() => void newFolder(open)}
          />
        </Tooltip>
        <Tooltip label={deleteLabel}>
          <IconButton
            size="sm"
            icon="trash-2"
            label={deleteLabel}
            disabled={nothingSelected}
            onClick={requestDelete}
          />
        </Tooltip>
        <span className="cw-media__spacer" />
        {allCards.length > 0 ? (
          <>
            {searching ? (
              <span className="cw-media__count caption numeric" role="status">
                {t("mediaBin.count", { shown: cards.length, total: allCards.length })}
              </span>
            ) : null}
            <Menu mode="dropdown" items={filterItems} aria-label={t("mediaBin.filters.menuLabel")}>
              <Tooltip label={filtersLabel}>
                <IconButton
                  size="sm"
                  icon="sliders-horizontal"
                  label={filtersLabel}
                  badge={filterCount}
                />
              </Tooltip>
            </Menu>
            {manualOpen ? (
              // A manual collection keeps the order its clips were added in.
              <Tooltip label={sortFixedLabel}>
                <IconButton size="sm" icon="arrow-up-down" label={sortFixedLabel} disabled />
              </Tooltip>
            ) : (
              <Menu mode="dropdown" items={sortItems} aria-label={t("mediaBin.sort.menuLabel")}>
                <Tooltip label={sortLabel}>
                  <IconButton size="sm" icon="arrow-up-down" label={sortLabel} />
                </Tooltip>
              </Menu>
            )}
          </>
        ) : null}
        <SegmentedControl
          className="cw-media__layout"
          aria-label={t("mediaBin.layout.label")}
          value={layout}
          iconOnly
          options={[
            { value: "grid", label: t("mediaBin.layout.grid"), icon: "layout-grid" },
            { value: "list", label: t("mediaBin.layout.list"), icon: "list" },
          ]}
          onChange={(next) => layoutStore.getState().set(next)}
        />
      </div>
      <div className="cw-media__row">
        {sidebarShown ? (
          <BinSidebar
            mode={sidebarLayout}
            open={open}
            collection={collection}
            dropOver={dropOver}
            dropOn={dropOn}
            onDone={closeOverlay}
          />
        ) : null}
        <div className="cw-media__main">
          {showBreadcrumb ? (
            <div className="cw-media__crumbs">
              <MediaBreadcrumb
                label={t("mediaBin.folder.navLabel")}
                segments={segments}
                dropTarget={dropOver}
                onNavigate={(id) => {
                  // The collection's own segment is where the bin is already.
                  if (id !== collection || id === null) openFolder(id as FolderId | null);
                }}
                onSegmentDragOver={(segment, e) => crumbDropFor(segment.id).onDragOver(e)}
                onSegmentDragLeave={(segment, e) => crumbDropFor(segment.id).onDragLeave(e)}
                onSegmentDrop={(segment, e) => crumbDropFor(segment.id).onDrop(e)}
              />
            </div>
          ) : null}
          {chips.length > 0 || canSaveSmart ? (
            <fieldset className="cw-media__chips" aria-label={t("mediaBin.filters.chipsLabel")}>
              {chips.map((chip) => (
                <ContextSurface
                  key={chip.id}
                  surface="bin.chip"
                  className="cw-context-surface"
                  target={() => ({ ids: [chip.id], name: chip.label })}
                >
                  <FilterChip
                    label={chip.label}
                    removeLabel={t("mediaBin.filters.removeChip", { filter: chip.label })}
                    onRemove={chip.remove}
                  />
                </ContextSurface>
              ))}
              {chips.length > 0 ? (
                <Button variant="ghost" size="sm" onClick={() => bin.clearFilters()}>
                  {t("mediaBin.filters.clearAll")}
                </Button>
              ) : null}
              {canSaveSmart ? (
                <Button variant="ghost" size="sm" onClick={() => openSaveSmartCollection()}>
                  {t("mediaBin.collections.saveSmart")}
                </Button>
              ) : null}
            </fieldset>
          ) : null}
          {nothingAtAll ? (
            <div className="cw-media__scroll">
              <EmptyState
                icon="film"
                title={t("assets.empty.media.title")}
                body={t("assets.empty.media.body")}
              />
            </div>
          ) : emptyCollection ? (
            <div className="cw-media__scroll">
              <EmptyState
                icon={manualOpen ? "layers" : "search"}
                title={t("mediaBin.collections.emptyTitle")}
                body={t(
                  manualOpen
                    ? "mediaBin.collections.emptyManual"
                    : "mediaBin.collections.emptySmart",
                )}
              />
            </div>
          ) : emptyFolder ? (
            <div className="cw-media__scroll">
              <EmptyState
                icon="folder"
                title={t("mediaBin.folder.emptyTitle")}
                body={t("mediaBin.folder.emptyBody")}
              />
            </div>
          ) : order.length === 0 ? (
            <div className="cw-media__scroll">
              <EmptyState
                icon="search"
                title={
                  text.trim() === ""
                    ? t("mediaBin.noMatch.titleFilters")
                    : t("mediaBin.noMatch.titleText", { text: text.trim() })
                }
                body={t("mediaBin.noMatch.body")}
              >
                <div className="cw-media__empty-actions">
                  {text.trim() === "" ? null : (
                    <Button variant="secondary" size="sm" onClick={() => bin.setText("")}>
                      {t("mediaBin.noMatch.clearSearch")}
                    </Button>
                  )}
                  {filterCount === 0 ? null : (
                    <Button variant="secondary" size="sm" onClick={() => bin.clearFilters()}>
                      {t("mediaBin.noMatch.clearFilters")}
                    </Button>
                  )}
                </div>
              </EmptyState>
            </div>
          ) : (
            <div ref={scroller} className="cw-media__scroll" onScroll={measure}>
              <div
                role="listbox"
                aria-label={t("mediaBin.listLabel")}
                aria-multiselectable="true"
                data-layout={layout}
                className="cw-media__list"
                onKeyDown={onKeyDown}
                // A double-click is not a slow click: it opens or inserts instead of renaming.
                onDoubleClickCapture={() => {
                  if (slowRename.current !== null) clearTimeout(slowRename.current);
                  slowRename.current = null;
                }}
                onFocus={(e) => {
                  const el = e.target as HTMLElement;
                  const media = el.closest("[data-media-id]")?.getAttribute("data-media-id");
                  const folder = el.closest("[data-folder-id]")?.getAttribute("data-folder-id");
                  const index = order.findIndex((tile) =>
                    media != null
                      ? tile.kind === "media" && tile.id === media
                      : tile.kind === "folder" && tile.id === folder,
                  );
                  if (index >= 0) setFocusIndex(index);
                }}
              >
                {folders.length > 0 ? (
                  <div className="cw-media__grid cw-media__grid--folders" data-layout={layout}>
                    {folders.map((folder, i) => (
                      <FolderView
                        key={folder.id}
                        folder={folder}
                        selected={selectedFolders.includes(folder.id)}
                        active={i === activeIndex}
                        renaming={renaming?.kind === "folder" && renaming.id === folder.id}
                        dropTarget={dropOver === folder.id}
                        path={pathOf(folder.parent)}
                        drop={dropOn(folder.id)}
                        onOpen={openFolderTile}
                        onRename={renameFolderTile}
                        {...handlers}
                      />
                    ))}
                  </div>
                ) : null}
                {cards.length > 0 ? (
                  <div
                    ref={grid}
                    data-layout={layout}
                    className="cw-media__grid"
                    style={{ paddingTop: win.padTop, paddingBottom: win.padBottom }}
                  >
                    {cards.slice(win.start, win.end).map((card, i) => (
                      <CardView
                        key={card.id}
                        card={card}
                        selected={selectedMedia.includes(card.id)}
                        active={nFolders + win.start + i === activeIndex}
                        analyzing={analyzing.has(card.id)}
                        failure={
                          failures[card.id]?.map((f) => `${f.stage}: ${f.error}`).join("\n") ?? null
                        }
                        path={pathOf(card.folder)}
                        renaming={renaming?.kind === "media" && renaming.id === card.id}
                        uses={usage.get(card.id) ?? 0}
                        onSelect={select}
                        onInsert={insert}
                        onRename={renameCard}
                        {...handlers}
                      />
                    ))}
                  </div>
                ) : null}
              </div>
            </div>
          )}
        </div>
      </div>
      {fileOver ? (
        <div className="cw-media__drop" aria-hidden="true">
          <Icon name="upload" size="lg" />
          <span className="body-strong">{t("mediaBin.drop.title")}</span>
          <span className="body">{t("mediaBin.drop.body")}</span>
        </div>
      ) : null}
    </section>
  );
}
