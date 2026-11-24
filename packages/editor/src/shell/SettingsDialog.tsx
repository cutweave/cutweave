// SPDX-License-Identifier: GPL-3.0-or-later
// File > Settings… (Ctrl+,; on macOS the app menu's Settings… with ⌘,): the app's own
// preferences. Appearance holds the theme, which used to sit in the title bar, where every pixel
// counts; Saving holds the autosave preference (WP-1M.10), which applies to every project and is
// sent to the engine as it changes. Changes apply at once and persist with the rest of the view
// state; later phases add the AI provider keys here (arch §13).
import { Button, Dialog, InspectorRow, SegmentedControl, Switch } from "@cutweave/ui";
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { desktopFromWindow } from "../session/desktop.ts";
import {
  AUTOSAVE_BACKUPS,
  AUTOSAVE_INTERVALS_S,
  THEME_LABEL_KEYS,
  THEMES,
  type ThemeId,
  useViewStore,
  viewStore,
} from "../state/viewStore.ts";
import { settingsStore, useSettingsOpen } from "./appCommands.ts";

const INTERVAL_LABELS: Record<(typeof AUTOSAVE_INTERVALS_S)[number], TKey> = {
  30: "settings.interval30s",
  60: "settings.interval1m",
  120: "settings.interval2m",
  300: "settings.interval5m",
};

export function SettingsDialog() {
  const { t } = useTranslation();
  const open = useSettingsOpen();
  const theme = useViewStore((s) => s.theme);
  const autosave = useViewStore((s) => s.autosave);
  // macOS: the native app menu's Settings… arrives as a menu command.
  useEffect(() => {
    const menu = desktopFromWindow()?.menu;
    if (menu === undefined) return;
    return menu.onCommand((command) => {
      if (command === "settings") settingsStore.getState().setOpen(true);
    });
  }, []);
  if (!open) return null;
  const close = (): void => settingsStore.getState().setOpen(false);
  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close();
      }}
      title={t("settings.title")}
      description={t("settings.description")}
      footer={
        <Button variant="primary" size="lg" onClick={close}>
          {t("settings.done")}
        </Button>
      }
    >
      <h3 className="cw-settings__section overline">{t("settings.appearance")}</h3>
      <InspectorRow label={t("settings.theme")}>
        <SegmentedControl<ThemeId>
          aria-label={t("settings.theme")}
          value={theme}
          onChange={(id) => viewStore.getState().setTheme(id)}
          options={THEMES.map((id) => ({ value: id, label: t(THEME_LABEL_KEYS[id]) }))}
        />
      </InspectorRow>
      <h3 className="cw-settings__section overline">{t("settings.saving")}</h3>
      <InspectorRow label={t("settings.autosave")} htmlFor="settings-autosave">
        <Switch
          id="settings-autosave"
          checked={autosave.enabled}
          onCheckedChange={(enabled) => viewStore.getState().setAutosave({ enabled })}
        />
      </InspectorRow>
      <InspectorRow label={t("settings.autosaveInterval")}>
        <SegmentedControl
          aria-label={t("settings.autosaveInterval")}
          value={String(autosave.intervalS)}
          disabled={!autosave.enabled}
          onChange={(id) => viewStore.getState().setAutosave({ intervalS: Number(id) })}
          options={AUTOSAVE_INTERVALS_S.map((seconds) => ({
            value: String(seconds),
            label: t(INTERVAL_LABELS[seconds]),
          }))}
        />
      </InspectorRow>
      <p className="cw-settings__hint caption">{t("settings.autosaveIntervalHint")}</p>
      <InspectorRow label={t("settings.keepBackups")}>
        <SegmentedControl
          aria-label={t("settings.keepBackups")}
          value={String(autosave.keepBackups)}
          disabled={!autosave.enabled}
          onChange={(id) => viewStore.getState().setAutosave({ keepBackups: Number(id) })}
          options={AUTOSAVE_BACKUPS.map((count) => ({
            value: String(count),
            label: String(count),
          }))}
        />
      </InspectorRow>
      <p className="cw-settings__hint caption">{t("settings.keepBackupsHint")}</p>
    </Dialog>
  );
}
