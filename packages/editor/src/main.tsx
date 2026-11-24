// SPDX-License-Identifier: GPL-3.0-or-later
import "@cutweave/ui/tokens/tokens.css";
import "@cutweave/ui/fonts/fonts.css";
import "dockview-react/dist/styles/dockview.css";
import "./app.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { engineStore } from "./engine/engineStore.ts";
import { bridgeFromWindow, startEngineRuntime } from "./engine/runtime.ts";
import { appBridgeFromWindow, installTestHandleIfEnabled } from "./engine/testHandle.ts";
import { i18n } from "./i18n/index.ts";
import { binStore } from "./media/binStore.ts";
import { mediaStore } from "./media/mediaStore.ts";
import { migrateLegacyView } from "./prefs/legacy.ts";
import { preferences, prefsStore } from "./prefs/prefsStore.ts";
import { desktopFromWindow } from "./session/desktop.ts";
import { sessionStore } from "./session/sessionStore.ts";
import { EditorShell } from "./shell/EditorShell.tsx";
import { docStore } from "./state/docStore.ts";
import { jobStore } from "./state/jobStore.ts";
import { playbackStore } from "./state/playbackStore.ts";
import { toastStore } from "./state/toastStore.ts";
import { viewStore } from "./state/viewStore.ts";

/** The browser's localStorage when it works (the view store's old home of the theme). */
function localStorageOrNull(): Storage | null {
  try {
    return globalThis.localStorage;
  } catch {
    return null;
  }
}

async function boot(): Promise<void> {
  const root = document.getElementById("root");
  if (!root) throw new Error("index.html is missing #root");

  // The preferences come first: the shell paints in the person's theme, and the engine hears the
  // saving preferences with its first handshake. Main answers from memory, so this is quick; a
  // preload without them (plain `vite` dev) takes the defaults at once.
  await preferences.attach(desktopFromWindow()?.prefs ?? null);
  // A profile from before the preferences store keeps its theme and saving choices (WP-1M.11).
  await migrateLegacyView(localStorageOrNull(), preferences);

  createRoot(root).render(
    <StrictMode>
      <EditorShell />
    </StrictMode>,
  );

  // Inside the desktop app the preload exposes the engine; in plain `vite` dev there is none and
  // the editor keeps its sample data.
  const bridge = bridgeFromWindow();
  if (bridge) {
    const stores = {
      prefs: prefsStore,
      doc: docStore,
      view: viewStore,
      playback: playbackStore,
      jobs: jobStore,
      media: mediaStore,
      bin: binStore,
      engine: engineStore,
      session: sessionStore,
    };
    const runtime = startEngineRuntime(bridge, stores, {
      // The engine could not autosave: say what it said and what to do (WP-1C.03, WP-1M.07).
      onAutosaveFailed: (message) =>
        toastStore.getState().push({
          tone: "warning",
          title: i18n.t("projectUi.autosaveFailedTitle"),
          message: i18n.t("projectUi.autosaveFailed", { message }),
        }),
    });
    // The e2e suites drive the editor's connection; only when the desktop app enables its hooks.
    void installTestHandleIfEnabled(appBridgeFromWindow(), runtime, stores);
  }
}

void boot();
