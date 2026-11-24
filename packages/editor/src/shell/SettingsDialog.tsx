// SPDX-License-Identifier: GPL-3.0-or-later
// File > Settings… (Ctrl+,; on macOS the app menu's Settings… with ⌘,): the frame of every
// settings page (WP-1M.11). A large modal in the main window (other windows are sandboxed and
// cannot reach the engine, arch §2): a search field over a vertical list of pages, one page of
// rows at a time. Pages and rows are registered data (src/settings/registry.ts); this file knows
// none of them. Changes apply at once; Done closes.

import {
  Button,
  Dialog,
  SearchField,
  SettingsLayout,
  type SettingsPageGroup,
  SettingsResultGroup,
} from "@cutweave/ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { useEngineReady } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { preferences, usePrefs } from "../prefs/prefsStore.ts";
import { desktopFromWindow } from "../session/desktop.ts";
import "../settings/core.ts";
import {
  type ConfirmSpec,
  pageKeys,
  SETTINGS_GROUPS,
  type SettingsContext,
  type SettingsPageDef,
  settingsRegistry,
} from "../settings/registry.ts";
import { SettingRowView } from "../settings/SettingsControls.tsx";
import { countMatches, searchSettings } from "../settings/search.ts";
import { openSettings, settingsStore, useSettingsOpen } from "../settings/settingsStore.ts";

/** Every page that has at least one row, in order. */
function useSettingsPages(): readonly SettingsPageDef[] {
  const pages = useStore(settingsRegistry, (s) => s.pages);
  return useMemo(() => pages.filter((p) => p.rows.length > 0), [pages]);
}

interface ConfirmRequest {
  spec: ConfirmSpec;
  resolve(answer: boolean): void;
}

/** Brings a row into view and puts the focus on its control (a deep link or a result). */
function focusRow(id: string): void {
  const row = [...document.querySelectorAll<HTMLElement>("[data-setting]")].find(
    (el) => el.dataset.setting === id,
  );
  if (row === undefined) return;
  row.scrollIntoView?.({ block: "nearest" });
  row
    .querySelector<HTMLElement>(
      'input:not([disabled]),select:not([disabled]),button[role="switch"]:not([disabled]),[role="radio"][aria-checked="true"]:not([disabled]),button:not([disabled])',
    )
    ?.focus();
}

function SettingsContent() {
  const { t } = useTranslation();
  const pages = useSettingsPages();
  const target = useStore(settingsStore, (s) => s.target);
  const values = usePrefs((s) => s.values);
  const engineReady = useEngineReady();
  const searchRef = useRef<HTMLInputElement>(null);
  // Only the target at the moment of opening decides the first page.
  const [pageId, setPageId] = useState(() => {
    const byRow =
      target.row === undefined
        ? undefined
        : pages.find((p) => p.rows.some((r) => r.id === target.row));
    return byRow?.id ?? pages.find((p) => p.id === target.page)?.id ?? pages[0]?.id ?? "";
  });
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState<string | undefined>(target.row);
  const [status, setStatus] = useState("");
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  // While the search field has the focus, Esc belongs to it (clear, then leave), not to the dialog.
  const [searchFocused, setSearchFocused] = useState(true);

  // A deep link lands on its row and puts the focus there, after the dialog has taken its own.
  useEffect(() => {
    if (target.row === undefined) return;
    const id = window.setTimeout(() => focusRow(target.row as string), 0);
    return () => window.clearTimeout(id);
  }, [target.row]);

  const ctx = useMemo<SettingsContext>(
    () => ({
      client: preferences,
      values,
      desktop: desktopFromWindow(),
      t: (key, vars) => t(key as never, vars as never) as unknown as string,
      say: setStatus,
      confirm: (spec) =>
        new Promise<boolean>((resolve) => {
          setConfirm({ spec, resolve });
        }),
    }),
    [t, values],
  );

  const searching = query.trim() !== "";
  const found = useMemo(
    () => searchSettings(pages, query, (key) => t(key as TKey) as string),
    [pages, query, t],
  );
  const page = pages.find((p) => p.id === pageId) ?? pages[0];

  const groups = useMemo<SettingsPageGroup[]>(
    () =>
      SETTINGS_GROUPS.map((group) => ({
        id: group.id,
        label: group.label === null ? undefined : (t(group.label as TKey) as string),
        pages: pages
          .filter((p) => p.group === group.id)
          .map((p) => ({ id: p.id, label: t(p.title as TKey) as string, icon: p.icon })),
      })).filter((group) => group.pages.length > 0),
    [pages, t],
  );

  const defaults = usePrefs((s) => s.defaults);
  const modifiedKeys =
    page === undefined
      ? []
      : pageKeys(page).filter(
          (key) => JSON.stringify(values[key]) !== JSON.stringify(defaults[key]),
        );

  const close = (): void => settingsStore.getState().setOpen(false);
  const matchCount = countMatches(found);
  const rowProps = { ctx, engineReady };

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close();
      }}
      title={t("settings.title")}
      description={t("settings.description")}
      className="cw-dialog--settings"
      dirty={searchFocused}
      footer={
        <>
          <p className="cw-settings__status caption" role="status">
            {status}
          </p>
          <Button variant="primary" size="lg" onClick={close}>
            {t("settings.done")}
          </Button>
        </>
      }
    >
      <SettingsLayout
        navLabel={t("settings.pagesLabel")}
        resultsLabel={t("settings.resultsLabel")}
        search={
          <SearchField
            ref={searchRef}
            value={query}
            onChange={(next) => {
              setQuery(next);
              setHighlight(undefined);
            }}
            label={t("settings.search")}
            clearLabel={t("settings.clearSearch")}
            count={searching ? t("settings.resultCount", { count: matchCount }) : undefined}
            // A second Esc leaves the field for the page list; the next one closes the dialog.
            onExit={() =>
              document.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus()
            }
            autoFocus
            onFocus={() => setSearchFocused(true)}
            onBlur={() => setSearchFocused(false)}
          />
        }
        groups={groups}
        activeId={searching ? null : (page?.id ?? null)}
        onSelect={(id) => {
          setPageId(id);
          setHighlight(undefined);
        }}
        heading={
          searching
            ? t("settings.resultsFor", { query: query.trim() })
            : page === undefined
              ? ""
              : t(page.title as TKey)
        }
        actions={
          !searching && modifiedKeys.length > 0 ? (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void preferences.reset(modifiedKeys).catch(() => undefined)}
            >
              {t("settings.resetPage")}
            </Button>
          ) : undefined
        }
      >
        {searching ? (
          found.length === 0 ? (
            <div className="cw-settings__nomatch">
              <p className="cw-settings__empty body">
                {t("settings.noMatch", { query: query.trim() })}
              </p>
              <Button size="sm" onClick={() => setQuery("")}>
                {t("settings.clearSearch")}
              </Button>
            </div>
          ) : (
            found.map((match) => (
              <SettingsResultGroup key={match.page.id} label={t(match.page.title as TKey)}>
                {match.rows.map((hit) => (
                  <SettingRowView
                    key={hit.row.id}
                    row={hit.row}
                    labelParts={hit.label}
                    descriptionParts={hit.description}
                    {...rowProps}
                  />
                ))}
              </SettingsResultGroup>
            ))
          )
        ) : (
          page?.rows.map((row) => (
            <SettingRowView
              key={row.id}
              row={row}
              highlighted={row.id === highlight}
              {...rowProps}
            />
          ))
        )}
      </SettingsLayout>
      {confirm === null ? null : (
        <Dialog
          open
          onOpenChange={(next) => {
            if (!next) {
              confirm.resolve(false);
              setConfirm(null);
            }
          }}
          title={t(confirm.spec.title as TKey)}
          description={t(confirm.spec.message as TKey)}
          footer={
            <>
              <Button
                variant="ghost"
                size="lg"
                onClick={() => {
                  confirm.resolve(false);
                  setConfirm(null);
                }}
              >
                {t("settings.cancel")}
              </Button>
              <Button
                variant={confirm.spec.danger ? "danger" : "primary"}
                size="lg"
                onClick={() => {
                  confirm.resolve(true);
                  setConfirm(null);
                }}
              >
                {t(confirm.spec.confirm as TKey)}
              </Button>
            </>
          }
        >
          {null}
        </Dialog>
      )}
    </Dialog>
  );
}

export function SettingsDialog() {
  const open = useSettingsOpen();
  const openCount = useStore(settingsStore, (s) => s.openCount);
  // macOS: the native app menu's Settings… arrives as a menu command.
  useEffect(() => {
    const menu = desktopFromWindow()?.menu;
    if (menu === undefined) return;
    return menu.onCommand((command) => {
      if (command === "settings") openSettings();
    });
  }, []);
  // Each opening starts afresh (empty search, the target's page); a deep link while it is open
  // moves to the new row.
  return open ? <SettingsContent key={openCount} /> : null;
}
