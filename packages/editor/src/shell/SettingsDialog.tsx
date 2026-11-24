// SPDX-License-Identifier: GPL-3.0-or-later
// File > Settings… (Ctrl+,; on macOS the app menu's Settings… with ⌘,): the app's own
// preferences. Appearance holds the theme, which used to sit in the title bar, where every pixel
// counts. Changes apply at once and persist with the rest of the view state; later phases add
// the AI provider keys here (arch §13).
import { Button, Dialog, InspectorRow, SegmentedControl } from "@cutweave/ui";
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { desktopFromWindow } from "../session/desktop.ts";
import {
  THEME_LABEL_KEYS,
  THEMES,
  type ThemeId,
  useViewStore,
  viewStore,
} from "../state/viewStore.ts";
import { settingsStore, useSettingsOpen } from "./appCommands.ts";

export function SettingsDialog() {
  const { t } = useTranslation();
  const open = useSettingsOpen();
  const theme = useViewStore((s) => s.theme);
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
    </Dialog>
  );
}
