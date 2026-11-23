// SPDX-License-Identifier: GPL-3.0-or-later
import { TooltipProvider } from "@cutweave/ui";
import { useEffect } from "react";
import { I18nextProvider, useTranslation } from "react-i18next";
import { Dock } from "../dock/Dock.tsx";
import { i18n } from "../i18n/index.ts";
import { useViewStore } from "../state/viewStore.ts";
import { Toasts } from "./Toasts.tsx";
import { TopBar } from "./TopBar.tsx";

function Shell() {
  const { t, i18n: instance } = useTranslation();
  const theme = useViewStore((s) => s.theme);

  // Tokens switch on `data-theme`; the compact density matches the reference screenshots.
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.density = "compact";
  }, [theme]);

  useEffect(() => {
    document.documentElement.lang = instance.language;
    document.title = t("app.windowTitle");
  }, [instance.language, t]);

  return (
    <TooltipProvider>
      <div className="cw-app">
        <TopBar />
        <main className="cw-app__dock">
          <Dock />
        </main>
      </div>
      <Toasts />
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
