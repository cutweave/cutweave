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
import { sessionStore } from "./session/sessionStore.ts";
import { EditorShell } from "./shell/EditorShell.tsx";
import { docStore } from "./state/docStore.ts";
import { jobStore } from "./state/jobStore.ts";
import { playbackStore } from "./state/playbackStore.ts";
import { toastStore } from "./state/toastStore.ts";
import { viewStore } from "./state/viewStore.ts";

const root = document.getElementById("root");
if (!root) throw new Error("index.html is missing #root");
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
    doc: docStore,
    view: viewStore,
    playback: playbackStore,
    jobs: jobStore,
    engine: engineStore,
    session: sessionStore,
  };
  const runtime = startEngineRuntime(bridge, stores, {
    // The engine could not autosave: say what it said and what to do (WP-1C.03, WP-1M.07).
    onAutosaveFailed: (message) =>
      toastStore.getState().push({
        tone: "warning",
        title: i18n.t("project.autosaveFailedTitle"),
        message: i18n.t("project.autosaveFailed", { message }),
      }),
  });
  // The e2e suites drive the editor's connection; only when the desktop app enables its hooks.
  void installTestHandleIfEnabled(appBridgeFromWindow(), runtime, stores);
}
