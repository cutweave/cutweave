// SPDX-License-Identifier: GPL-3.0-or-later
// The History panel (WP-2H.11, ADR-0019): every change to the project, oldest at the top (or
// newest, a per-user choice, #171), who made it and when, a marker under the current state and the undone changes dimmed below it.
// Click a row to go back (or forward) to the state right after it. Agent rows are agent blue
// (design/README.md). The list is windowed: only the rows in view are in the page.

import { Icon, IconButton, type IconName, SegmentedControl, Tooltip } from "@cutweave/ui";
import {
  type KeyboardEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useEngineStore } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { useContextMenu } from "../menus/host.tsx";
import { EmptyState } from "../panels/EmptyState.tsx";
import { preferences, usePref } from "../prefs/prefsStore.ts";
import { actorDisplayName, historyLabel } from "../session/historyLabels.ts";
import { type Translate, translator } from "../timeline/errors.ts";
import { goToStep, HistoryFeed } from "./historyFeed.ts";
import {
  buildRows,
  type HistoryFilter,
  type HistoryRow,
  markerIndex,
  matchesFilter,
  relativeTime,
} from "./historyModel.ts";
import { historyStore, useHistoryStore } from "./historyStore.ts";

/** Rows drawn beyond the visible ones, so scrolling never shows a gap. */
const OVERSCAN = 6;
/** How often "2 min ago" is recomputed. */
const CLOCK_MS = 30_000;

const KIND_ICON: Record<HistoryRow["kind"], IconName> = {
  user: "user",
  agent: "sparkles",
  plugin: "plug",
  system: "cpu",
};

type Item = { readonly kind: "row"; readonly row: HistoryRow } | { readonly kind: "marker" };

/** The list, with the marker between the applied rows and the undone tail. */
function itemsOf(rows: readonly HistoryRow[]): Item[] {
  const at = markerIndex(rows);
  const items: Item[] = rows.map((row) => ({ kind: "row", row }));
  items.splice(at, 0, { kind: "marker" });
  return items;
}

/** "You", the agent's label, the plugin's id. */
export function whoOf(row: HistoryRow, t: Translate): string {
  return row.kind === "user" ? t("history.panel.you") : (actorDisplayName(row.actor, t) ?? "");
}

const AGO = {
  minute: "history.panel.ago.minute",
  hour: "history.panel.ago.hour",
  day: "history.panel.ago.day",
} as const satisfies Record<string, TKey>;

export function whenText(at: number, now: number, t: Translate): string {
  const rel = relativeTime(at, now);
  if (rel.unit === "now") return t("history.panel.ago.now");
  const key = AGO[rel.unit];
  return (t as unknown as (key: TKey, vars: { count: number }) => string)(key, {
    count: rel.count,
  });
}

function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), CLOCK_MS);
    return () => clearInterval(id);
  }, []);
  return now;
}

interface RowViewProps {
  readonly row: HistoryRow;
  readonly now: number;
  readonly dim: boolean;
  readonly active: boolean;
  readonly busy: boolean;
  readonly onGo: (txId: string) => void;
}

function RowView({ row, now, dim, active, busy, onGo }: RowViewProps) {
  const { t, i18n } = useTranslation();
  const translate = translator(t);
  const step = historyLabel(row.label, translate);
  const who = whoOf(row, translate);
  const when = whenText(row.at, now, translate);
  const undone = row.state === "undone";
  const label = undone
    ? t("history.panel.rowUndone", { step, who, when })
    : t("history.panel.rowLabel", { step, who, when });
  const ref = useContextMenu<HTMLButtonElement>("history.row", () => ({
    ids: [row.txId],
    name: step,
    data: { txId: row.txId, label: step },
  }));
  const exact = new Date(row.at).toLocaleString(i18n.language);
  return (
    <li className="cw-history__item">
      <Tooltip label={exact} side="left">
        <button
          ref={ref}
          type="button"
          className="cw-history__row"
          data-tx={row.txId}
          data-actor={row.kind}
          data-state={row.state}
          data-dim={dim ? "true" : undefined}
          tabIndex={active ? 0 : -1}
          aria-label={label}
          aria-disabled={busy ? true : undefined}
          onClick={() => {
            if (!busy) onGo(row.txId);
          }}
        >
          <Icon name={KIND_ICON[row.kind]} size="sm" className="cw-history__icon" />
          <span className="cw-history__step body">{step}</span>
          <span className="cw-history__who caption">{who}</span>
          <span className="cw-history__when caption">{when}</span>
        </button>
      </Tooltip>
    </li>
  );
}

/** The panel. */
export function HistoryPanel() {
  const { t } = useTranslation();
  const rpc = useEngineStore((s) => s.rpc);
  const ready = useEngineStore((s) => s.connection === "ready");
  const entries = useHistoryStore((s) => s.entries);
  const busy = useHistoryStore((s) => s.busy);
  const notice = useHistoryStore((s) => s.notice);
  const now = useNow();
  const [filter, setFilter] = useState<HistoryFilter>("all");
  const [activeTx, setActiveTx] = useState<string | null>(null);

  // The feed lives as long as the panel is open and the engine is connected.
  useEffect(() => {
    if (rpc === null || !ready) return;
    const feed = new HistoryFeed({ rpc });
    feed.start();
    return () => feed.stop();
  }, [rpc, ready]);
  useEffect(() => () => historyStore.getState().setNotice(null), []);

  const rows = useMemo(() => buildRows(entries), [entries]);
  const order = usePref("history.order");
  const newestFirst = order === "newest";
  // The marker moves with the rows: newest first, the undone tail is on top and the marker under it.
  const items = useMemo(() => {
    const list = itemsOf(rows);
    return newestFirst ? list.reverse() : list;
  }, [rows, newestFirst]);
  const markerAt = useMemo(() => items.findIndex((i) => i.kind === "marker"), [items]);

  // ---- windowing ----
  const listRef = useRef<HTMLDivElement | null>(null);
  const [view, setView] = useState({ top: 0, height: 0, rowHeight: 0 });
  const measure = useCallback(() => {
    const el = listRef.current;
    if (el === null) return;
    const first = el.querySelector<HTMLElement>(".cw-history__item, .cw-history__marker");
    setView((v) => {
      const next = {
        top: el.scrollTop,
        height: el.clientHeight,
        rowHeight: first?.offsetHeight ?? v.rowHeight,
      };
      return next.top === v.top && next.height === v.height && next.rowHeight === v.rowHeight
        ? v
        : next;
    });
  }, []);
  useLayoutEffect(measure);
  useEffect(() => {
    const el = listRef.current;
    if (el === null || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [measure]);

  const windowed = view.rowHeight > 0 && view.height > 0;
  const first = windowed ? Math.max(0, Math.floor(view.top / view.rowHeight) - OVERSCAN) : 0;
  const last = windowed
    ? Math.min(items.length, Math.ceil((view.top + view.height) / view.rowHeight) + OVERSCAN)
    : items.length;

  // Keep the marker in view when it moves (a new change, an undo, a jump).
  const lastMarker = useRef<{ at: number; count: number } | null>(null);
  useLayoutEffect(() => {
    const el = listRef.current;
    const previous = lastMarker.current;
    lastMarker.current = { at: markerAt, count: items.length };
    if (el === null || view.rowHeight === 0 || markerAt < 0) return;
    if (previous !== null && previous.at === markerAt && previous.count === items.length) return;
    const top = markerAt * view.rowHeight;
    const bottom = top + view.rowHeight;
    if (top < el.scrollTop || bottom > el.scrollTop + el.clientHeight) {
      el.scrollTop = Math.max(0, bottom - el.clientHeight + view.rowHeight * 2);
    }
  });

  // ---- keyboard ----
  // In the order on screen: the arrow keys follow the list, Home and End are its first and last row.
  const rowIds = useMemo(
    () => items.flatMap((i) => (i.kind === "row" ? [i.row.txId] : [])),
    [items],
  );
  const fallbackActive = useMemo(() => {
    const at = markerIndex(rows);
    return rows[Math.max(0, at - 1)]?.txId ?? null;
  }, [rows]);
  const current = activeTx !== null && rowIds.includes(activeTx) ? activeTx : fallbackActive;
  const focusWanted = useRef(false);
  useLayoutEffect(() => {
    if (!focusWanted.current || current === null) return;
    const el = listRef.current?.querySelector<HTMLElement>(`[data-tx="${CSS.escape(current)}"]`);
    if (el === null || el === undefined) return;
    focusWanted.current = false;
    el.focus({ preventScroll: false });
  });

  const move = (to: number) => {
    const id = rowIds[Math.min(rowIds.length - 1, Math.max(0, to))];
    if (id === undefined) return;
    focusWanted.current = true;
    setActiveTx(id);
    // A row outside the window is not in the page yet: scroll it in, the layout effect focuses it.
    const el = listRef.current;
    if (el !== null && view.rowHeight > 0) {
      const index = items.findIndex((i) => i.kind === "row" && i.row.txId === id);
      const top = index * view.rowHeight;
      if (top < el.scrollTop) el.scrollTop = top;
      else if (top + view.rowHeight > el.scrollTop + el.clientHeight) {
        el.scrollTop = top + view.rowHeight - el.clientHeight;
      }
    }
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const at = current === null ? -1 : rowIds.indexOf(current);
    if (event.key === "ArrowDown") move(at + 1);
    else if (event.key === "ArrowUp") move(at - 1);
    else if (event.key === "Home") move(0);
    else if (event.key === "End") move(rowIds.length - 1);
    else return;
    event.preventDefault();
  };

  const go = (txId: string) => {
    setActiveTx(txId);
    void goToStep(rows, txId);
  };

  if (rows.length === 0) {
    return (
      <div className="cw-history">
        <EmptyState
          icon="history"
          title={t("history.panel.emptyTitle")}
          body={t("history.panel.emptyBody")}
        />
      </div>
    );
  }
  const shown = items.slice(first, last);
  const sortLabel = newestFirst ? t("history.panel.sortOldest") : t("history.panel.sortNewest");
  return (
    <div className="cw-history">
      <div className="cw-history__tools">
        <SegmentedControl<HistoryFilter>
          aria-label={t("history.panel.filterLabel")}
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: t("history.panel.filter.all") },
            { value: "user", label: t("history.panel.filter.user") },
            { value: "agent", label: t("history.panel.filter.agent") },
          ]}
        />
        <Tooltip label={sortLabel}>
          <IconButton
            size="sm"
            icon="arrow-up-down"
            label={sortLabel}
            onClick={() => void preferences.set("history.order", newestFirst ? "oldest" : "newest")}
          />
        </Tooltip>
      </div>
      {notice !== null ? (
        <div className="cw-history__notice" role="status">
          <div className="cw-history__notice-text">
            <p className="body-strong">{t("history.panel.stoppedTitle")}</p>
            <p className="caption">{notice}</p>
          </div>
          <IconButton
            size="sm"
            icon="x"
            label={t("history.panel.dismiss")}
            onClick={() => historyStore.getState().setNotice(null)}
          />
        </div>
      ) : null}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: the keys move between the row buttons inside */}
      <div className="cw-history__scroll" ref={listRef} onScroll={measure} onKeyDown={onKeyDown}>
        <ul
          className="cw-history__list"
          aria-label={t("history.panel.label")}
          style={
            windowed
              ? {
                  paddingBlockStart: first * view.rowHeight,
                  paddingBlockEnd: (items.length - last) * view.rowHeight,
                }
              : undefined
          }
        >
          {shown.map((item) =>
            item.kind === "marker" ? (
              <li key="marker" className="cw-history__marker" aria-current="step">
                <span className="cw-history__marker-line" aria-hidden="true" />
                <span className="cw-history__marker-text caption">{t("history.panel.here")}</span>
              </li>
            ) : (
              <RowView
                key={item.row.txId}
                row={item.row}
                now={now}
                dim={!matchesFilter(item.row, filter)}
                active={item.row.txId === current}
                busy={busy}
                onGo={go}
              />
            ),
          )}
        </ul>
      </div>
    </div>
  );
}
