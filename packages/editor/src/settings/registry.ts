// SPDX-License-Identifier: GPL-3.0-or-later
// The settings page registry (WP-1M.11). Pages and their rows are data; a work package adds a
// page, or a row to a page, by registering it, with no change to the dialog:
//
//   registerSettingsPage({ id: "export", title: <a catalog key>, icon: "download",
//     group: "preferences", order: 80, rows: [ … ] });
//   registerSettingRow("saving", { id: "saving.something", key: "saving.something", … });
//
// A page appears in the dialog once it has a row. Every row that edits a preference names its
// key, and a test checks that each registered key has a row in docs/reference/settings.md.

import type { PreferenceKey, PreferenceMap } from "@cutweave/preferences";
import type { IconName } from "@cutweave/ui";
import type { ReactNode } from "react";
import { createStore } from "zustand/vanilla";
import type { TKey } from "../i18n/index.ts";
import type { PreferencesClient } from "../prefs/prefsStore.ts";
import type { DesktopBridge } from "../session/desktop.ts";

/** The groups of the page list, in the order the catalog shows them. */
export const SETTINGS_GROUPS = [
  { id: "preferences", label: "settings.groups.preferences" },
  { id: "agents", label: "settings.groups.agents" },
  { id: "extensions", label: "settings.groups.extensions" },
  { id: "privacy", label: "settings.groups.privacy" },
  { id: "machine", label: "settings.groups.machine" },
  // Advanced stands alone, with no heading.
  { id: "advanced", label: null },
] as const satisfies readonly { id: string; label: TKey | null }[];
export type SettingsGroupId = (typeof SETTINGS_GROUPS)[number]["id"];

export interface ConfirmSpec {
  title: TKey;
  message: TKey;
  confirm: TKey;
  /** The action cannot be undone: the confirm button is red. */
  danger?: boolean;
}

/** What a row's action, status and custom control can use. */
export interface SettingsContext {
  readonly client: PreferencesClient;
  readonly values: PreferenceMap;
  readonly desktop: DesktopBridge | null;
  readonly t: (key: TKey, vars?: Record<string, unknown>) => string;
  /** A line under the rows, read out politely: "Imported 3 settings." */
  say(message: string): void;
  /** Asks the person to confirm; resolves to their answer. */
  confirm(spec: ConfirmSpec): Promise<boolean>;
}

/** A row's name: its own text, or its catalog key translated. */
export const rowLabel = (row: SettingRowDef, t: (key: TKey) => string): string =>
  row.labelText ?? t(row.label);

/** A row's description: its own text, or its catalog key translated. */
export const rowDescription = (row: SettingRowDef, t: (key: TKey) => string): string =>
  row.descriptionText ?? t(row.description);

/** One choice: its words are a catalog key, or plain text for numbers and names. */
export interface ChoiceOption {
  value: string | number;
  label?: TKey;
  text?: string;
}

export type ControlSpec =
  | { kind: "switch" }
  | { kind: "segmented"; options: readonly ChoiceOption[] }
  | { kind: "select"; options: readonly ChoiceOption[] }
  | { kind: "number"; min: number; max: number; unit?: TKey }
  /** A folder, chosen with the OS folder picker. */
  | { kind: "folder"; choose: TKey }
  /** Items, each removable, with an optional add button. */
  | {
      kind: "list";
      items(ctx: SettingsContext): readonly { id: string; label: string }[];
      remove(ctx: SettingsContext, id: string): void | Promise<void>;
      empty: TKey;
      removeLabel: TKey;
      add?: { label: TKey; run(ctx: SettingsContext): void | Promise<void> };
    }
  | {
      kind: "action";
      label: TKey;
      run(ctx: SettingsContext): void | Promise<void>;
      /** A short status shown beside the button ("3 stored"). */
      status?(ctx: SettingsContext): string | undefined;
      /** The button does nothing now. */
      disabled?(ctx: SettingsContext): boolean;
      danger?: boolean;
    }
  | { kind: "custom"; render(ctx: SettingsContext, controlId: string): ReactNode };

/** Who acts on a value: it applies at once, needs the engine, or waits for the next start. */
export type AppliedBy = "now" | "engine" | "restart";

export interface SettingRowDef {
  /** Unique within its page; the preference key for a row that edits one. */
  id: string;
  /** The preference this row edits (it is checked against the catalog). */
  key?: PreferenceKey;
  label: TKey;
  description: TKey;
  /** Plain words instead of the catalog keys, for a row built from data (a command's summary). */
  labelText?: string;
  descriptionText?: string;
  /** Extra words that find the row, comma separated, in the UI language. */
  keywords?: TKey;
  /** More words that find the row, worked out when searching (the keys of a shortcut). */
  keywordsText?: () => string;
  /** A heading the page groups this row under: consecutive rows with the same one share it. */
  section?: TKey;
  control: ControlSpec;
  applies?: AppliedBy;
  /** The row cannot be changed now (the host says why in its note when it is the engine). */
  disabledWhen?: (values: PreferenceMap) => boolean;
  /** The row's own control already resets it (an action): no reset button beside it. */
  noRowReset?: boolean;
}

export interface SettingsPageDef {
  id: string;
  title: TKey;
  icon: IconName;
  group: SettingsGroupId;
  /** Position among the pages of the group. */
  order: number;
  /** A line under the page's heading. */
  note?: TKey;
  /** The page has its own "find" field above its rows (a long page, like the shortcut list). */
  filterable?: boolean;
  rows: readonly SettingRowDef[];
}

interface RegistryState {
  pages: readonly SettingsPageDef[];
  /** Rows registered before their page: they join it when it appears. */
  pending: Readonly<Record<string, readonly SettingRowDef[]>>;
}

export const settingsRegistry = createStore<RegistryState>()(() => ({ pages: [], pending: {} }));

const groupIndex = (id: SettingsGroupId): number => SETTINGS_GROUPS.findIndex((g) => g.id === id);

function sorted(pages: readonly SettingsPageDef[]): SettingsPageDef[] {
  return [...pages].sort((a, b) => groupIndex(a.group) - groupIndex(b.group) || a.order - b.order);
}

/** Adds a page (or replaces the one with the same id). Returns a function that removes it. */
export function registerSettingsPage(page: SettingsPageDef): () => void {
  settingsRegistry.setState((s) => {
    const extra = s.pending[page.id] ?? [];
    const { [page.id]: _taken, ...pending } = s.pending;
    const rows = [...page.rows, ...extra.filter((r) => !page.rows.some((p) => p.id === r.id))];
    return {
      pages: sorted([...s.pages.filter((p) => p.id !== page.id), { ...page, rows }]),
      pending,
    };
  });
  return () =>
    settingsRegistry.setState((s) => ({ pages: s.pages.filter((p) => p.id !== page.id) }));
}

/** Adds a row to a page, now or when the page registers. Returns a function that removes it. */
export function registerSettingRow(pageId: string, row: SettingRowDef): () => void {
  settingsRegistry.setState((s) => {
    const page = s.pages.find((p) => p.id === pageId);
    if (page === undefined) {
      return { pending: { ...s.pending, [pageId]: [...(s.pending[pageId] ?? []), row] } };
    }
    return {
      pages: s.pages.map((p) =>
        p.id === pageId ? { ...p, rows: [...p.rows.filter((r) => r.id !== row.id), row] } : p,
      ),
    };
  });
  return () =>
    settingsRegistry.setState((s) => ({
      pages: s.pages.map((p) =>
        p.id === pageId ? { ...p, rows: p.rows.filter((r) => r.id !== row.id) } : p,
      ),
      pending: {
        ...s.pending,
        [pageId]: (s.pending[pageId] ?? []).filter((r) => r.id !== row.id),
      },
    }));
}

/** The preference keys a page's rows edit. */
export function pageKeys(page: SettingsPageDef): PreferenceKey[] {
  return page.rows.flatMap((row) => (row.key ? [row.key] : []));
}

/** The pages that have a row, in order. */
export function settingsPages(): readonly SettingsPageDef[] {
  return settingsRegistry.getState().pages.filter((p) => p.rows.length > 0);
}

/** Every preference key a registered row edits. */
export function registeredKeys(): PreferenceKey[] {
  return settingsPages().flatMap((p) => p.rows.flatMap((r) => (r.key ? [r.key] : [])));
}
