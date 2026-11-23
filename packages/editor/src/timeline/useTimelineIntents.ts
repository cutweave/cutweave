// SPDX-License-Identifier: GPL-3.0-or-later
// `useTimelineIntents()` (WP-1M.06 interface): one `TimelineController` per timeline panel, bound
// to the app's stores, with the callbacks the panel hands to `TimelineCanvas`.
import type { TimelinePointerEvent, TimelineView } from "@cutweave/timeline";
import { useCallback, useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { engineStore } from "../engine/engineStore.ts";
import { docStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { TimelineController, type TimelineOverlay } from "./controller.ts";
import { type Translate, translator } from "./errors.ts";

export interface TimelineIntents {
  readonly controller: TimelineController;
  /** Snap guides and marquee to draw. */
  readonly overlay: TimelineOverlay;
  /** `TimelineCanvas.onPointer`. */
  readonly onPointer: (e: TimelinePointerEvent) => void;
  /** `TimelineCanvas.onView`. */
  readonly onView: (view: TimelineView | null) => void;
  /** Ref for the panel root: timeline keys (S, N, Delete, Backspace, Esc) pressed inside it. */
  readonly keyTarget: (el: HTMLElement | null) => (() => void) | undefined;
}

/** Keys typed into a field belong to the field. */
function isTextEntry(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (el === null || typeof el.tagName !== "string") return false;
  return el.isContentEditable || el.tagName === "INPUT" || el.tagName === "TEXTAREA";
}

export function useTimelineIntents(): TimelineIntents {
  const { t } = useTranslation();
  const translate = useRef<Translate>(translator(t));
  translate.current = translator(t);

  const controller = useMemo(
    () =>
      new TimelineController({
        doc: docStore,
        view: viewStore,
        engine: engineStore,
        toasts: toastStore,
        t: (key, vars) => translate.current(key, vars),
      }),
    [],
  );

  // Losing the window mid-drag cancels the gesture (the pointer-up may never arrive).
  useEffect(() => {
    const blur = (): void => controller.cancel();
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("blur", blur);
      controller.dispose();
    };
  }, [controller]);

  const overlay = useStore(controller.overlay);

  const onPointer = useCallback(
    (e: TimelinePointerEvent) => {
      // Keys go where the pointer acted: focus the canvas on a press.
      if (e.type === "down") (e.original.target as HTMLElement | null)?.focus?.();
      controller.handlePointer(e);
    },
    [controller],
  );

  const onView = useCallback((view: TimelineView | null) => controller.attach(view), [controller]);

  const keyTarget = useCallback(
    (el: HTMLElement | null) => {
      if (el === null) return undefined;
      const onKey = (e: KeyboardEvent): void => {
        if (e.defaultPrevented || isTextEntry(e.target)) return;
        if (controller.handleKey(e)) e.preventDefault();
      };
      el.addEventListener("keydown", onKey);
      return () => el.removeEventListener("keydown", onKey);
    },
    [controller],
  );

  return { controller, overlay, onPointer, onView, keyTarget };
}
