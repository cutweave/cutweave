// SPDX-License-Identifier: GPL-3.0-or-later
import type { MediaId } from "@cutweave/protocol";
import type { MediaDrag } from "@cutweave/timeline";
import {
  Button,
  Dialog,
  FilterChip,
  Icon,
  IconButton,
  type IconName,
  Menu,
  type MenuEntry,
  SearchField,
  SegmentedControl,
  StatusChip,
  Tooltip,
} from "@cutweave/ui";
import {
  type DragEvent,
  type KeyboardEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { engineStore, useEngineStore } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import {
  activeFilterCount,
  BIN_KINDS,
  type BinKind,
  binStore,
  DURATION_BUCKET_IDS,
  type DurationBucket,
  SORT_CHOICES,
  toMediaFilter,
  useBinStore,
} from "../media/binStore.ts";
import {
  formatDuration,
  type MediaCard,
  selectMediaCards,
  selectMediaUsage,
} from "../media/cards.ts";
import { dragStore, MEDIA_DRAG_TYPE } from "../media/dragStore.ts";
import { findMedia } from "../media/find.ts";
import { insertAtPlayhead } from "../media/insert.ts";
import { useMediaStore } from "../media/mediaStore.ts";
import { removeMedia } from "../media/remove.ts";
import { useThumbnailSource } from "../media/thumbnails.ts";
import { useCardScrub } from "../media/useCardScrub.ts";
import { useImport } from "../media/useImport.ts";
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

type MediaLayout = "grid" | "list";

/** The modifier key cap for tooltips: ⌘ on macOS, Ctrl elsewhere. */
function modifierKey(): string {
  const platform = typeof navigator === "undefined" ? "" : navigator.platform;
  return /mac/i.test(platform) ? "⌘" : "Ctrl";
}

/** Session-wide, like the other panel toggles: the layout survives switching asset tabs. */
const layoutStore = createStore<{ layout: MediaLayout; set(layout: MediaLayout): void }>()(
  (set) => ({ layout: "grid", set: (layout) => set({ layout }) }),
);

const KIND_ICON: Record<MediaCard["kind"], IconName> = {
  video: "film",
  audio: "audio-waveform",
  image: "image",
  sequence_ref: "layers",
};

const KIND_KEY: Record<MediaCard["kind"], TKey> = {
  video: "mediaBin.kind.video",
  audio: "mediaBin.kind.audio",
  image: "mediaBin.kind.image",
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
function windowOf(count: number, m: Metrics) {
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

const dragOf = (card: MediaCard): MediaDrag | null =>
  card.kind === "sequence_ref"
    ? null
    : { id: card.id, kind: card.kind, duration: card.kind === "image" ? null : card.duration };

interface CardViewProps {
  card: MediaCard;
  selected: boolean;
  /** The one card in the tab order (roving tabindex). */
  active: boolean;
  analyzing: boolean;
  failure: string | null;
  onSelect(id: MediaId): void;
  onInsert(card: MediaCard): void;
}

/** Whether a card can play on its own (ADR-0022): sound and video; stills have nothing to play. */
const playable = (card: MediaCard): boolean =>
  (card.kind === "video" || card.kind === "audio") &&
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
  onSelect,
  onInsert,
}: CardViewProps) {
  const { t } = useTranslation();
  const source = useThumbnailSource();
  const indexed = useMediaStore((s) => s.indexed[card.id] ?? 0);
  const [failedImage, setFailedImage] = useState<string | null>(null);
  const thumb = source.thumbnailFor({ id: card.id, kind: card.kind, width: 160, indexed });
  const image = thumb.kind === "image" && thumb.src !== failedImage ? thumb.src : null;
  const drag = dragOf(card);
  const scrub = useCardScrub(card);
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

  const onDragStart = (e: DragEvent<HTMLElement>) => {
    if (drag === null) {
      e.preventDefault();
      return;
    }
    scrub.stop();
    onSelect(card.id);
    e.dataTransfer.setData(MEDIA_DRAG_TYPE, card.id);
    e.dataTransfer.effectAllowed = "copy";
    dragStore.getState().begin(drag);
  };

  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: the listbox handles the keys (arrows, Enter, Space, Delete)
    <div
      role="option"
      aria-selected={selected}
      tabIndex={active ? 0 : -1}
      draggable={drag !== null}
      data-media-id={card.id}
      data-kind={card.kind}
      className="cw-media__card"
      onClick={() => onSelect(card.id)}
      onDoubleClick={() => onInsert(card)}
      onDragStart={onDragStart}
      onDragEnd={() => dragStore.getState().end()}
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
      </span>
      <span className="cw-media__body">
        <span className="cw-media__name body-strong">{card.name}</span>
        <span className="cw-media__meta caption">
          <span>{t(KIND_KEY[card.kind])}</span>
          {meta === "" ? null : <span>{meta}</span>}
          {card.codec === null ? null : <span className="cw-media__badge">{card.codec}</span>}
        </span>
        {card.broken || failure !== null || analyzing ? (
          <span className="cw-media__status" title={failure ?? undefined}>
            {card.broken ? (
              <StatusChip tone="danger">{t("mediaBin.status.broken")}</StatusChip>
            ) : failure !== null ? (
              <StatusChip tone="warning">{t("mediaBin.status.analysisFailed")}</StatusChip>
            ) : (
              <StatusChip tone="neutral">{t("mediaBin.status.analyzing")}</StatusChip>
            )}
          </span>
        ) : null}
      </span>
    </div>
  );
}

/**
 * The media bin (WP-1M.03): the project's assets as a grid or list, imported from a native dialog
 * (Ctrl+I) or by dropping files from the OS, dragged onto the timeline, or added at the playhead
 * with Enter. Listbox semantics with a roving tab stop: arrows move (and select), Enter inserts,
 * Delete removes.
 */
export function MediaPanel() {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const allCards = useMemo(() => selectMediaCards(project), [project]);
  const usage = useMemo(() => selectMediaUsage(project), [project]);
  const text = useBinStore((s) => s.text);
  const filters = useBinStore((s) => s.filters);
  const sort = useBinStore((s) => s.sort);
  // The bin shows what the engine's `media.list` would list for the same filter and sort (the
  // same predicate, run on the mirror: no round trip per keystroke).
  const cards = useMemo(
    () => findMedia(allCards, usage, toMediaFilter({ text, filters }), sort).cards,
    [allCards, usage, text, filters, sort],
  );
  const filterCount = activeFilterCount(filters);
  const searching = text.trim() !== "" || filterCount > 0;
  const selection = useViewStore((s) => s.selection);
  const selectedId = (selection.find((r) => r.kind === "media")?.id ?? null) as MediaId | null;
  const layout = useStore(layoutStore, (s) => s.layout);
  const ready = useEngineStore((s) => s.rpc !== null && s.connection === "ready");
  const failures = useMediaStore((s) => s.failures);
  const jobs = useJobStore((s) => s.jobs);
  const importer = useImport();

  const analyzing = useMemo(() => {
    const ids = new Set<string>();
    for (const job of Object.values(jobs)) {
      if (
        job.kind === "analysis" &&
        job.media_id !== null &&
        (job.state === "queued" || job.state === "running")
      ) {
        ids.add(job.media_id);
      }
    }
    return ids;
  }, [jobs]);

  // ---- focus and windowing ---------------------------------------------------------------
  const scroller = useRef<HTMLDivElement>(null);
  const grid = useRef<HTMLDivElement>(null);
  const [metrics, setMetrics] = useState<Metrics>({ top: 0, height: 0, cols: 0, rowHeight: 0 });
  const [focusIndex, setFocusIndex] = useState(0);
  const focusRequest = useRef(false);

  const selectedIndex = cards.findIndex((c) => c.id === selectedId);
  // The tab stop follows focus; a selection made elsewhere (the inspector, another click) moves it.
  useEffect(() => {
    if (selectedIndex >= 0) setFocusIndex(selectedIndex);
  }, [selectedIndex]);
  const activeIndex = Math.min(Math.max(focusIndex, 0), Math.max(cards.length - 1, 0));

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
    setMetrics((m) =>
      m.top === el.scrollTop &&
      m.height === el.clientHeight &&
      m.cols === cols &&
      m.rowHeight === rowHeight
        ? m
        : { top: el.scrollTop, height: el.clientHeight, cols, rowHeight },
    );
  }, []);

  // Measure after every layout-affecting change; follow the scroller's own size too.
  useLayoutEffect(measure, [measure, cards, layout]);
  useEffect(() => {
    const el = scroller.current;
    if (el === null || typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [measure]);

  const win = windowOf(cards.length, layout === "list" ? { ...metrics, cols: 1 } : metrics);

  // Give the keyboard focus to the card a key press moved to, once it is drawn.
  useEffect(() => {
    if (!focusRequest.current) return;
    focusRequest.current = false;
    const id = cards[activeIndex]?.id;
    const el =
      id === undefined ? null : grid.current?.querySelector<HTMLElement>(`[data-media-id="${id}"]`);
    el?.focus();
  });

  const select = useCallback((id: MediaId) => {
    viewStore.getState().setSelection([{ kind: "media", id }]);
  }, []);

  const goTo = (index: number) => {
    const card = cards[index];
    const el = scroller.current;
    if (card === undefined || el === null) return;
    setFocusIndex(index);
    select(card.id);
    focusRequest.current = true;
    if (metrics.rowHeight > 0 && metrics.cols > 0) {
      const cols = layout === "list" ? 1 : metrics.cols;
      const rowTop = Math.floor(index / cols) * metrics.rowHeight;
      if (rowTop < el.scrollTop) el.scrollTop = rowTop;
      else if (rowTop + metrics.rowHeight > el.scrollTop + el.clientHeight) {
        el.scrollTop = rowTop + metrics.rowHeight - el.clientHeight;
      }
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

  const insert = useCallback(
    (card: MediaCard) => {
      const drag = dragOf(card);
      if (drag !== null) void insertAtPlayhead(actionDeps, drag);
    },
    [actionDeps],
  );

  const [confirming, setConfirming] = useState<MediaCard | null>(null);
  const removing = async (card: MediaCard) => {
    setConfirming(null);
    await removeMedia(actionDeps, card.id, card.name);
  };
  const requestRemove = (card: MediaCard | undefined) => {
    if (card === undefined) return;
    if ((usage.get(card.id) ?? 0) > 0) setConfirming(card);
    else void removing(card);
  };
  const selectedCard = cards.find((c) => c.id === selectedId);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    // The keys are the table in binKeys.ts (Settings > Keyboard shortcuts lists the same one).
    const cols = layout === "list" ? 1 : Math.max(metrics.cols, 1);
    const last = cards.length - 1;
    let next: number | null = null;
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
        const card = cards[activeIndex];
        if (card !== undefined) insert(card);
        e.preventDefault();
        return;
      }
      case "play": {
        // Space selects the focused card, as before, then plays or pauses it in the Source view.
        const card = cards[activeIndex];
        if (card !== undefined) {
          select(card.id);
          if (playable(card)) playCard(card);
        }
        e.preventDefault();
        return;
      }
      case "remove":
        requestRemove(cards[activeIndex]);
        e.preventDefault();
        return;
      default:
        return;
    }
    e.preventDefault();
    if (next !== null) goTo(Math.min(Math.max(next, 0), last));
  };

  // Ctrl/Cmd+I is announced by the empty state: it opens the file dialog from anywhere in the app.
  const pickRef = useRef(importer.pickAndImport);
  pickRef.current = importer.pickAndImport;
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (binKeyAction(e) !== "import") return;
      if (e.defaultPrevented || isTextEntry(e.target)) return;
      e.preventDefault();
      void pickRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // ---- OS file drop ----------------------------------------------------------------------
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
  const onDrop = (e: DragEvent<HTMLElement>) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    setFileOver(false);
    const paths = importer.pathsOf(Array.from(e.dataTransfer.files));
    if (paths === null) {
      toastStore
        .getState()
        .push({ tone: "info", title: tr.current("mediaBin.import.desktopOnly") });
      return;
    }
    void importer.importPaths(paths);
  };

  // ---- search, filters, sort ---------------------------------------------------------------
  const searchRef = useRef<HTMLInputElement>(null);
  const sectionRef = useRef<HTMLElement>(null);

  // Ctrl/Cmd+F focuses the search field while the Assets panel has focus.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (binKeyAction(e) !== "search") return;
      const root = sectionRef.current?.closest(".cw-assets") ?? sectionRef.current;
      if (e.defaultPrevented || root === null || !root.contains(document.activeElement)) return;
      e.preventDefault();
      searchRef.current?.focus();
      searchRef.current?.select();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const bin = binStore.getState();
  const durationLabel = (id: DurationBucket) => t(DURATION_KEY[id]);
  const kindLabel = (kind: BinKind) => t(KIND_KEY[kind]);
  const useLabel = (use: "used" | "unused") => t(USE_KEY[use]);

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
  ];

  const importLabel = t("mediaBin.importButton");
  const filtersLabel =
    filterCount > 0
      ? t("mediaBin.filters.buttonActive", { count: filterCount })
      : t("mediaBin.filters.button");
  const sortLabel = t("mediaBin.sort.button");
  return (
    <section
      ref={sectionRef}
      className="cw-media"
      aria-label={t("mediaBin.panelLabel")}
      data-drop-active={fileOver ? "true" : undefined}
      onDragOver={onDragOver}
      onDragEnter={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      {allCards.length > 0 ? (
        <div className="cw-media__search">
          <SearchField
            ref={searchRef}
            value={text}
            onChange={(next) => bin.setText(next)}
            label={t("mediaBin.search.label")}
            clearLabel={t("mediaBin.search.clear")}
            onArrowDown={() => goTo(0)}
          />
        </div>
      ) : null}
      <div className="cw-media__tools">
        {/* Icons with tooltips: the panel is narrow and its toolbar is for doing, not reading. */}
        <Tooltip label={importLabel} keys={[modifierKey(), "I"]}>
          <IconButton
            size="sm"
            icon="upload"
            label={importLabel}
            disabled={!ready || importer.busy}
            onClick={() => void importer.pickAndImport()}
          />
        </Tooltip>
        <Tooltip label={t("mediaBin.remove.button")}>
          <IconButton
            size="sm"
            icon="trash-2"
            label={t("mediaBin.remove.button")}
            disabled={selectedCard === undefined}
            onClick={() => requestRemove(selectedCard)}
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
            <Menu mode="dropdown" items={sortItems} aria-label={t("mediaBin.sort.menuLabel")}>
              <Tooltip label={sortLabel}>
                <IconButton size="sm" icon="arrow-up-down" label={sortLabel} />
              </Tooltip>
            </Menu>
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
      {chips.length > 0 ? (
        <fieldset className="cw-media__chips" aria-label={t("mediaBin.filters.chipsLabel")}>
          {chips.map((chip) => (
            <FilterChip
              key={chip.id}
              label={chip.label}
              removeLabel={t("mediaBin.filters.removeChip", { filter: chip.label })}
              onRemove={chip.remove}
            />
          ))}
          <Button variant="ghost" size="sm" onClick={() => bin.clearFilters()}>
            {t("mediaBin.filters.clearAll")}
          </Button>
        </fieldset>
      ) : null}
      {allCards.length === 0 ? (
        <div className="cw-media__scroll">
          <EmptyState
            icon="film"
            title={t("assets.empty.media.title")}
            body={t("assets.empty.media.body")}
          />
        </div>
      ) : cards.length === 0 ? (
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
            ref={grid}
            role="listbox"
            aria-label={t("mediaBin.listLabel")}
            data-layout={layout}
            className="cw-media__grid"
            style={{ paddingTop: win.padTop, paddingBottom: win.padBottom }}
            onKeyDown={onKeyDown}
            onFocus={(e) => {
              const id = (e.target as HTMLElement)
                .closest("[data-media-id]")
                ?.getAttribute("data-media-id");
              const index =
                id === null || id === undefined ? -1 : cards.findIndex((c) => c.id === id);
              if (index >= 0) setFocusIndex(index);
            }}
          >
            {cards.slice(win.start, win.end).map((card, i) => (
              <CardView
                key={card.id}
                card={card}
                selected={card.id === selectedId}
                active={win.start + i === activeIndex}
                analyzing={analyzing.has(card.id)}
                failure={failures[card.id]?.map((f) => `${f.stage}: ${f.error}`).join("\n") ?? null}
                onSelect={select}
                onInsert={insert}
              />
            ))}
          </div>
        </div>
      )}
      {fileOver ? (
        <div className="cw-media__drop" aria-hidden="true">
          <Icon name="upload" size="lg" />
          <span className="body-strong">{t("mediaBin.drop.title")}</span>
          <span className="body">{t("mediaBin.drop.body")}</span>
        </div>
      ) : null}
      <Dialog
        open={confirming !== null}
        onOpenChange={(open) => {
          if (!open) setConfirming(null);
        }}
        title={t("mediaBin.remove.confirmTitle", { name: confirming?.name ?? "" })}
        description={t("mediaBin.remove.confirmBody", {
          count: String(usage.get(confirming?.id ?? "") ?? 0),
        })}
        footer={
          <>
            <Button variant="ghost" size="lg" onClick={() => setConfirming(null)}>
              {t("mediaBin.remove.cancel")}
            </Button>
            <Button
              variant="danger"
              size="lg"
              onClick={() => confirming !== null && void removing(confirming)}
            >
              {t("mediaBin.remove.confirm")}
            </Button>
          </>
        }
      >
        <p className="body">{t("mediaBin.remove.confirmNote")}</p>
      </Dialog>
    </section>
  );
}
