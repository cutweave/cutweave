// SPDX-License-Identifier: GPL-3.0-or-later
import { resolveTheme } from "@cutweave/preferences";
import { ToastProvider, TooltipProvider } from "@cutweave/ui";
import { useEffect, useSyncExternalStore } from "react";
import { I18nextProvider, useTranslation } from "react-i18next";
import { Dock } from "../dock/Dock.tsx";
import { ExportDialog } from "../export/ExportDialog.tsx";
import { useExportWiring } from "../export/useExport.ts";
import { i18n } from "../i18n/index.ts";
import { useKeymapDispatcher } from "../keymap/dispatch.ts";
import { RelinkDialog } from "../media/RelinkDialog.tsx";
import { ContextMenuHost } from "../menus/host.tsx";
import { MenuDialogs } from "../menus/MenuDialogs.tsx";
import { CommandPalette } from "../palette/index.ts";
import { usePref } from "../prefs/prefsStore.ts";
import { desktopFromWindow } from "../session/desktop.ts";
import { useProjectWiring, useWindowTitle } from "../session/useProject.ts";
import { useTransportWiring } from "../transport/useTransport.ts";
import { useSuggestShortForm } from "../workspace/suggestShortForm.ts";
import { AboutDialog } from "./AboutDialog.tsx";
import { EngineNotices } from "./EngineNotices.tsx";
import { ProjectDialogs, ReadOnlyBanner } from "./ProjectDialogs.tsx";
import { SettingsDialog } from "./SettingsDialog.tsx";
import { Toasts } from "./Toasts.tsx";
import { TopBar } from "./TopBar.tsx";

const DARK_QUERY = "(prefers-color-scheme: dark)";

/** Whether the OS is in dark mode (Electron's `nativeTheme` follows it for `Match system`). */
function useSystemDark(): boolean {
  return useSyncExternalStore(
    (notify) => {
      const query = globalThis.matchMedia?.(DARK_QUERY);
      query?.addEventListener("change", notify);
      return () => query?.removeEventListener("change", notify);
    },
    () => globalThis.matchMedia?.(DARK_QUERY).matches ?? true,
    () => true,
  );
}

function Shell() {
  const { t, i18n: instance } = useTranslation();
  // The theme is an app preference (Settings > Appearance); `Match system` follows the OS live.
  const choice = usePref("appearance.theme");
  const theme = resolveTheme(choice, useSystemDark());

  // Tokens switch on `data-theme`; the compact density matches the reference screenshots.
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.density = "compact";
    // The OS window controls over the title bar take the same theme (Windows, Linux).
    void desktopFromWindow()
      ?.window?.setTheme?.(theme)
      .catch(() => undefined);
  }, [theme]);

  useEffect(() => {
    document.documentElement.lang = instance.language;
  }, [instance.language]);

  // Project actions: the native menu (macOS), the window-close handshake and the title
  // (WP-1M.07). The menu bar in the top bar has the shortcuts elsewhere (WP-1M.09).
  useProjectWiring();
  // Export jobs, their toasts and the native Export… command (WP-1M.08).
  useExportWiring();
  useWindowTitle();
  // Transport keys through the keymap; the timeline's playhead follows playback (WP-1M.04).
  useTransportWiring();
  // A vertical sequence in Long-form: offer Short-form, once per project (WP-2H.01).
  useSuggestShortForm();
  // Ctrl/Cmd+K opens the command palette (WP-2H.04).
  useKeymapDispatcher();

  return (
    <TooltipProvider>
      <ToastProvider label={t("app.notifications")}>
        <div className="cw-app">
          <TopBar />
          <ReadOnlyBanner />
          <main className="cw-app__dock">
            <Dock />
          </main>
        </div>
        <EngineNotices />
        <ProjectDialogs />
        <ExportDialog />
        <AboutDialog />
        <SettingsDialog />
        <CommandPalette />
        <Toasts />
        <MenuDialogs />
        <RelinkDialog />
        <ContextMenuHost />
      </ToastProvider>
    </TooltipProvider>
  );
}

/** The editor frame: top bar over the dockable panel area. Runs without an engine (WP-1M.01). */
export function EditorShell() {
  return (
    <I18nextProvider i18n={i18n}>
      <Shell />
    </I18nextProvider>
  );
}
