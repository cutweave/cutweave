// SPDX-License-Identifier: GPL-3.0-or-later
// The pages WP-1M.11 ships: General, Appearance, Saving and backups, Advanced. The other pages
// of docs/reference/settings.md arrive with their features (WP-1M.13 Export and Keyboard
// shortcuts, WP-1J.05 Agents and MCP and Notifications, WP-1D.08 Cache and storage and
// Performance and hardware, and later ones) and register themselves the same way.

import { AUTOSAVE_BACKUPS, AUTOSAVE_INTERVALS_S, RECENT_COUNT_MAX } from "@cutweave/preferences";
import { sessionUiStore } from "../session/sessionUiStore.ts";
// The "This computer" pages register themselves (WP-1D.08).
import "./machine.tsx";
import {
  type ChoiceOption,
  registerSettingsPage,
  type SettingsContext,
  type SettingsPageDef,
} from "./registry.ts";

const INTERVAL_LABELS = {
  30: "settings.rows.interval.s30",
  60: "settings.rows.interval.m1",
  120: "settings.rows.interval.m2",
  300: "settings.rows.interval.m5",
} as const;

const intervalOptions: readonly ChoiceOption[] = AUTOSAVE_INTERVALS_S.map((seconds) => ({
  value: seconds,
  label: INTERVAL_LABELS[seconds],
}));
const backupOptions: readonly ChoiceOption[] = AUTOSAVE_BACKUPS.map((count) => ({
  value: count,
  text: String(count),
}));

const autosaveOff = (values: { "saving.autosave": boolean }): boolean => !values["saving.autosave"];

/** What a skipped import line reads: `ghost (unknown setting)`. */
export function describeSkipped(skipped: readonly { key: string; reason: string }[]): string {
  return skipped.map((s) => `${s.key} (${s.reason})`).join(", ");
}

async function exportSettings(ctx: SettingsContext): Promise<void> {
  const result = await ctx.client.exportToFile();
  if (result.status === "saved") {
    ctx.say(ctx.t("settings.notices.exported", { count: result.count, path: result.path }));
  }
}

async function importSettings(ctx: SettingsContext): Promise<void> {
  const result = await ctx.client.importFromFile();
  if (result.status === "cancelled") return;
  if (result.status === "invalid") {
    ctx.say(
      ctx.t(
        result.reason === "unreadable"
          ? "settings.notices.unreadable"
          : result.reason === "newer"
            ? "settings.notices.newer"
            : "settings.notices.notSettings",
      ),
    );
    return;
  }
  const applied = ctx.t("settings.notices.imported", { count: result.applied.length });
  ctx.say(
    result.skipped.length === 0
      ? applied
      : `${applied} ${ctx.t("settings.notices.skipped", {
          count: result.skipped.length,
          list: describeSkipped(result.skipped),
        })}`,
  );
}

async function clearRecent(ctx: SettingsContext): Promise<void> {
  await ctx.desktop?.recent?.clear?.();
  sessionUiStore.getState().setRecent([]);
  ctx.say(ctx.t("settings.notices.recentCleared"));
}

async function resetAll(ctx: SettingsContext): Promise<void> {
  const yes = await ctx.confirm({
    title: "settings.rows.resetAll.confirmTitle",
    message: "settings.rows.resetAll.confirmMessage",
    confirm: "settings.rows.resetAll.confirm",
    danger: true,
  });
  if (!yes) return;
  await ctx.client.resetAll();
  ctx.say(ctx.t("settings.notices.resetAll"));
}

const general: SettingsPageDef = {
  id: "general",
  title: "settings.pages.general",
  icon: "settings",
  group: "preferences",
  order: 10,
  rows: [
    {
      id: "general.startup",
      key: "general.startup",
      label: "settings.rows.startup.label",
      description: "settings.rows.startup.description",
      keywords: "settings.rows.startup.keywords",
      control: {
        kind: "segmented",
        options: [
          { value: "empty", label: "settings.rows.startup.empty" },
          { value: "last", label: "settings.rows.startup.last" },
        ],
      },
      applies: "restart",
    },
    {
      id: "general.new_project_folder",
      key: "general.new_project_folder",
      label: "settings.rows.newProjectFolder.label",
      description: "settings.rows.newProjectFolder.description",
      keywords: "settings.rows.newProjectFolder.keywords",
      control: { kind: "folder", choose: "settings.rows.newProjectFolder.choose" },
    },
    {
      id: "general.recent_count",
      key: "general.recent_count",
      label: "settings.rows.recentCount.label",
      description: "settings.rows.recentCount.description",
      keywords: "settings.rows.recentCount.keywords",
      control: {
        kind: "number",
        min: 0,
        max: RECENT_COUNT_MAX,
        unit: "settings.rows.recentCount.unit",
      },
    },
    {
      id: "general.workspace",
      key: "general.workspace",
      label: "settings.rows.workspace.label",
      description: "settings.rows.workspace.description",
      keywords: "settings.rows.workspace.keywords",
      control: {
        kind: "segmented",
        options: [
          { value: "last", label: "settings.rows.workspace.last" },
          { value: "long-form", label: "workspace.long-form" },
          { value: "short-form", label: "workspace.short-form" },
        ],
      },
    },
    {
      id: "history.order",
      key: "history.order",
      label: "settings.rows.historyOrder.label",
      description: "settings.rows.historyOrder.description",
      keywords: "settings.rows.historyOrder.keywords",
      control: {
        kind: "segmented",
        options: [
          { value: "oldest", label: "settings.rows.historyOrder.oldest" },
          { value: "newest", label: "settings.rows.historyOrder.newest" },
        ],
      },
    },
    {
      id: "general.clear_recent",
      label: "settings.rows.clearRecent.label",
      description: "settings.rows.clearRecent.description",
      keywords: "settings.rows.clearRecent.keywords",
      control: {
        kind: "action",
        label: "settings.rows.clearRecent.button",
        run: clearRecent,
      },
    },
    {
      id: "general.dialog_choices",
      key: "general.dialog_choices",
      label: "settings.rows.dialogChoices.label",
      description: "settings.rows.dialogChoices.description",
      keywords: "settings.rows.dialogChoices.keywords",
      noRowReset: true,
      control: {
        kind: "action",
        label: "settings.rows.dialogChoices.button",
        run: (ctx) => ctx.client.reset(["general.dialog_choices"]),
        disabled: (ctx) => Object.keys(ctx.values["general.dialog_choices"]).length === 0,
        status: (ctx) => {
          const count = Object.keys(ctx.values["general.dialog_choices"]).length;
          return count === 0
            ? ctx.t("settings.rows.dialogChoices.none")
            : ctx.t("settings.rows.dialogChoices.stored", { count });
        },
      },
    },
  ],
};

const appearance: SettingsPageDef = {
  id: "appearance",
  title: "settings.pages.appearance",
  icon: "palette",
  group: "preferences",
  order: 20,
  rows: [
    {
      id: "appearance.theme",
      key: "appearance.theme",
      label: "settings.rows.theme.label",
      description: "settings.rows.theme.description",
      keywords: "settings.rows.theme.keywords",
      control: {
        kind: "segmented",
        options: [
          { value: "dark", label: "theme.dark" },
          { value: "light", label: "theme.light" },
          { value: "system", label: "theme.system" },
        ],
      },
    },
  ],
};

/** The rows WP-1M.10 built, on the preferences store: main hands them to the engine. */
const saving: SettingsPageDef = {
  id: "saving",
  title: "settings.pages.saving",
  icon: "history",
  group: "preferences",
  order: 90,
  rows: [
    {
      id: "saving.autosave",
      key: "saving.autosave",
      label: "settings.rows.autosave.label",
      description: "settings.rows.autosave.description",
      keywords: "settings.rows.autosave.keywords",
      control: { kind: "switch" },
      applies: "engine",
    },
    {
      id: "saving.interval",
      key: "saving.interval",
      label: "settings.rows.interval.label",
      description: "settings.rows.interval.description",
      keywords: "settings.rows.interval.keywords",
      control: { kind: "segmented", options: intervalOptions },
      applies: "engine",
      disabledWhen: autosaveOff,
    },
    {
      id: "saving.backups",
      key: "saving.backups",
      label: "settings.rows.backups.label",
      description: "settings.rows.backups.description",
      keywords: "settings.rows.backups.keywords",
      control: { kind: "segmented", options: backupOptions },
      applies: "engine",
      disabledWhen: autosaveOff,
    },
  ],
};

const advanced: SettingsPageDef = {
  id: "advanced",
  title: "settings.pages.advanced",
  icon: "sliders-horizontal",
  group: "advanced",
  order: 10,
  rows: [
    {
      id: "advanced.export",
      label: "settings.rows.exportSettings.label",
      description: "settings.rows.exportSettings.description",
      keywords: "settings.rows.exportSettings.keywords",
      control: {
        kind: "action",
        label: "settings.rows.exportSettings.button",
        run: exportSettings,
      },
    },
    {
      id: "advanced.import",
      label: "settings.rows.importSettings.label",
      description: "settings.rows.importSettings.description",
      keywords: "settings.rows.importSettings.keywords",
      control: {
        kind: "action",
        label: "settings.rows.importSettings.button",
        run: importSettings,
      },
    },
    {
      id: "advanced.reset_all",
      label: "settings.rows.resetAll.label",
      description: "settings.rows.resetAll.description",
      keywords: "settings.rows.resetAll.keywords",
      control: {
        kind: "action",
        label: "settings.rows.resetAll.button",
        run: resetAll,
        danger: true,
      },
    },
    {
      id: "advanced.open_settings_folder",
      label: "settings.rows.openSettingsFolder.label",
      description: "settings.rows.openSettingsFolder.description",
      keywords: "settings.rows.openSettingsFolder.keywords",
      control: {
        kind: "action",
        label: "settings.rows.openSettingsFolder.button",
        run: (ctx) => ctx.desktop?.shell?.openAppFolder?.("settings"),
        disabled: (ctx) => ctx.desktop?.shell?.openAppFolder === undefined,
      },
    },
    {
      id: "advanced.open_logs_folder",
      label: "settings.rows.openLogsFolder.label",
      description: "settings.rows.openLogsFolder.description",
      keywords: "settings.rows.openLogsFolder.keywords",
      control: {
        kind: "action",
        label: "settings.rows.openLogsFolder.button",
        run: (ctx) => ctx.desktop?.shell?.openAppFolder?.("logs"),
        disabled: (ctx) => ctx.desktop?.shell?.openAppFolder === undefined,
      },
    },
  ],
};

export const CORE_PAGES: readonly SettingsPageDef[] = [general, appearance, saving, advanced];

/** Registers the pages of this work package (idempotent). */
export function registerCorePages(): void {
  for (const page of CORE_PAGES) registerSettingsPage(page);
}
registerCorePages();
