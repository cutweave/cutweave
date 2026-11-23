// SPDX-License-Identifier: GPL-3.0-or-later
// `useTimelineIntents()` (WP-1M.06 interface): one `TimelineController` per timeline panel, bound
// to the app's stores, with the callbacks the panel hands to `TimelineCanvas`.
import type { TimelinePointerEvent, TimelineView } from "@cutweave/timeline";
import { type DragEvent, useCallback, useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { engineStore } from "../engine/engineStore.ts";
import { dragStore } from "../media/dragStore.ts";
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
  /** Handlers for the element around the canvas: a media card dragged from the bin onto the lanes. */
  readonly drop: {
    readonly onDragOver: (e: DragEvent<HTMLElement>) => void;
    readonly onDragLeave: (e: DragEvent<HTMLElement>) => void;
    readonly onDrop: (e: DragEvent<HTMLElement>) => void;
  };
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

  // A card drag that ends without a drop here (Esc, dropped elsewhere) clears the drop guide.
  useEffect(
    () =>
      dragStore.subscribe((state) => {
        if (state.session === null) controller.mediaDragLeave();
      }),
    [controller],
  );

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

  const drop = useMemo<TimelineIntents["drop"]>(() => {
    /** Canvas-relative CSS pixels of a drag event, or `null` without a canvas. */
    const local = (e: DragEvent<HTMLElement>): { x: number; y: number } | null => {
      const canvas = e.currentTarget.querySelector("canvas");
      if (canvas === null) return null;
      const r = canvas.getBoundingClientRect();
      return { x: e.clientX - r.left, y: e.clientY - r.top };
    };
    return {
      onDragOver(e) {
        const media = dragStore.getState().session;
        const at = media === null ? null : local(e);
        if (media === null || at === null) return;
        const lands = controller.mediaDragOver(at.x, at.y, media);
        // Allowing the drop is what shows the "copy" cursor; with nowhere to land it stays "no drop".
        if (lands) {
          e.preventDefault();
          e.dataTransfer.dropEffect = "copy";
        }
      },
      onDragLeave(e) {
        // Moving between children fires leave too: only a real exit clears the preview.
        const next = e.relatedTarget as Node | null;
        if (next === null || !e.currentTarget.contains(next)) controller.mediaDragLeave();
      },
      onDrop(e) {
        const media = dragStore.getState().session;
        const at = media === null ? null : local(e);
        if (media === null || at === null) return;
        e.preventDefault();
        controller.mediaDrop(at.x, at.y, media);
        dragStore.getState().end();
      },
    };
  }, [controller]);

  return { controller, overlay, onPointer, onView, keyTarget, drop };
}
