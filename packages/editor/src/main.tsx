// SPDX-License-Identifier: GPL-3.0-or-later
import "@cutweave/ui/tokens/tokens.css";
import "@cutweave/ui/fonts/fonts.css";
import "dockview-react/dist/styles/dockview.css";
import "./app.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { engineStore } from "./engine/engineStore.ts";
import { bridgeFromWindow, startEngineRuntime } from "./engine/runtime.ts";
import { EditorShell } from "./shell/EditorShell.tsx";
import { docStore } from "./state/docStore.ts";
import { jobStore } from "./state/jobStore.ts";
import { playbackStore } from "./state/playbackStore.ts";
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
  startEngineRuntime(bridge, {
    doc: docStore,
    view: viewStore,
    playback: playbackStore,
    jobs: jobStore,
    engine: engineStore,
  });
}
