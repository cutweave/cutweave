// SPDX-License-Identifier: GPL-3.0-or-later
// The app's transport controller over the global stores, and the shell wiring that binds it: the
// transport keys (through the keymap) and the playhead following `playback.state`.
import { useEffect } from "react";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { prefsStore } from "../prefs/prefsStore.ts";
import { docStore } from "../state/docStore.ts";
import { playbackStore } from "../state/playbackStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { keymap, useKeymapListener } from "./keymap.ts";
import { TransportController } from "./transport.ts";

let shared: TransportController | null = null;

/** The app's transport, created on first use. */
export function transportController(): TransportController {
  shared ??= new TransportController({
    engine: engineStore,
    playback: playbackStore,
    view: viewStore,
    doc: docStore,
    // Settings > Playback (WP-2H.10): read at each stop, so a change applies at once.
    returnToStart: () => prefsStore.getState().values["playback.return_to_start"],
    onUnsupported: () =>
      toastStore.getState().push({
        tone: "info",
        title: i18n.t("transport.unsupportedTitle"),
        message: i18n.t("transport.unsupported"),
      }),
    onError: (error) => console.warn("[transport]", error),
  });
  return shared;
}

/**
 * Mount once (the shell): window keys go through the keymap, the transport actions are
 * registered there, and the timeline's playhead follows the engine's.
 */
export function useTransportWiring(): void {
  useKeymapListener();
  useEffect(() => {
    const transport = transportController();
    const offs = [
      transport.attach(),
      keymap.register("transport:togglePlay", () => void transport.togglePlay()),
      keymap.register("transport:reverse", () => void transport.shuttle("J")),
      keymap.register("transport:pause", () => void transport.shuttle("K")),
      keymap.register("transport:forward", () => void transport.shuttle("L")),
      keymap.register("transport:stepBack", () => void transport.step(-1)),
      keymap.register("transport:stepForward", () => void transport.step(1)),
    ];
    return () => {
      for (const off of offs) off();
    };
  }, []);
}
