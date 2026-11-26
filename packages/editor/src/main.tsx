// SPDX-License-Identifier: GPL-3.0-or-later
import "@cutweave/ui/tokens/tokens.css";
import "@cutweave/ui/fonts/fonts.css";
import "dockview-react/dist/styles/dockview.css";
import "./app.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { agentToast } from "./agents/agentNames.ts";
import { AgentNotices, agentsStore } from "./agents/agentsStore.ts";
import { engineStore } from "./engine/engineStore.ts";
import { bridgeFromWindow, startEngineRuntime } from "./engine/runtime.ts";
import { appBridgeFromWindow, installTestHandleIfEnabled } from "./engine/testHandle.ts";
import { i18n } from "./i18n/index.ts";
import { MathStartupError } from "./math/MathStartupError.tsx";
import { startMath } from "./math/startup.ts";
import { binStore } from "./media/binStore.ts";
import { mediaStore } from "./media/mediaStore.ts";
import { startMediaHttp } from "./media/mediaWiring.ts";
import { migrateLegacyView } from "./prefs/legacy.ts";
import { preferences, prefsStore } from "./prefs/prefsStore.ts";
import { desktopFromWindow } from "./session/desktop.ts";
import { openNoticeToast } from "./session/openNotices.ts";
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

  // The shared math module before anything that converts a time, draws a curve or snaps (arch
  // §10, ADR-0033). A module that does not match this build stops the editor here.
  const math = await startMath(() => import("virtual:cutweave-math-wasm"));
  if (!math.ok) {
    createRoot(root).render(
      <StrictMode>
        <MathStartupError failure={math} />
      </StrictMode>,
    );
    return;
  }

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
      agents: agentsStore,
    };
    // A notice names an agent when it connects or disconnects (Settings > Notifications).
    new AgentNotices({
      notify: (notice) => {
        if (prefsStore.getState().values["notify.agent_connected"]) {
          toastStore
            .getState()
            .push(
              agentToast(
                notice,
                (key, vars) => i18n.t(key as never, vars as never) as unknown as string,
              ),
            );
        }
      },
    }).watch(agentsStore);
    const runtime = startEngineRuntime(bridge, stores, {
      // The engine could not autosave: say what it said and what to do (WP-1C.03, WP-1M.07).
      onAutosaveFailed: (message) =>
        toastStore.getState().push({
          tone: "warning",
          title: i18n.t("projectUi.autosaveFailedTitle"),
          message: i18n.t("projectUi.autosaveFailed", { message }),
        }),
      // An agent opened a project, or could not because of unsaved changes (ADR-0029).
      onOpenNotice: (notice) =>
        toastStore
          .getState()
          .push(
            openNoticeToast(
              notice,
              (key, vars) => i18n.t(key as never, vars as never) as unknown as string,
            ),
          ),
    });
    // Real thumbnails, filmstrips and waveforms once the engine is up and has media (WP-1I.03).
    startMediaHttp({ bridge, engine: engineStore, doc: docStore, jobs: jobStore });
    // The e2e suites drive the editor's connection; only when the desktop app enables its hooks.
    void installTestHandleIfEnabled(appBridgeFromWindow(), runtime, stores);
  }
}

void boot();
