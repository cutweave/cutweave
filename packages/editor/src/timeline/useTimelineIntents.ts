// SPDX-License-Identifier: GPL-3.0-or-later
// `useTimelineIntents()` (WP-1M.06 interface): one `TimelineController` per timeline panel, bound
// to the app's stores, with the callbacks the panel hands to `TimelineCanvas`.
import type { TimelinePointerEvent, TimelineView } from "@cutweave/timeline";
import { type DragEvent, useCallback, useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { engineStore } from "../engine/engineStore.ts";
import { timelineActionOf, timelineEditorCommandOf } from "../keymap/timeline.ts";
import { dragStore } from "../media/dragStore.ts";
import { requestForCanvasEvent } from "../menus/canvas.ts";
import { closeContextMenu, openContextMenu } from "../menus/host.tsx";
import { registerTimelineCanvas } from "../menus/timelineSurface.ts";
import { forEveryWindow, framesOf } from "../shell/windows.ts";
import { docStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { transportController } from "../transport/useTransport.ts";
import { setActiveTimelineController } from "./activeController.ts";
import {
  type Call,
  CLIPBOARD_MIME,
  clipboardStore,
  copyClips,
  cutClips,
  duplicateClips,
  isWritingClipboard,
  parsePayload,
  pasteClips,
  selectedClips,
} from "./clipboard.ts";
import { TimelineController, type TimelineOverlay } from "./controller.ts";
import {
  describeRefusal,
  FAILED_TITLE,
  type TimelineEdit,
  type Translate,
  translator,
} from "./errors.ts";
import { toRpcError } from "./gesture.ts";

export interface TimelineIntents {
  readonly controller: TimelineController;
  /** Snap guides and marquee to draw. */
  readonly overlay: TimelineOverlay;
  /** `TimelineCanvas.onPointer`. */
  readonly onPointer: (e: TimelinePointerEvent) => void;
  /** Ref for the element around the canvas: Shift+F10 and the Menu key open its context menu. */
  readonly canvasSurface: (el: HTMLElement | null) => (() => void) | undefined;
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

/** Calls the engine for the clipboard actions (the same calls a menu choice makes). */
const engineCall: Call = async (name, args) => {
  const rpc = engineStore.getState().rpc;
  if (rpc === null) throw new Error("the engine is not connected");
  return (rpc.call as (n: string, a: unknown) => Promise<unknown>)(name, args ?? {});
};

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
  /** The canvas's surface element: its window is where the gesture's frames come from. */
  const surface = useRef<HTMLElement | null>(null);

  const controller = useMemo(
    () =>
      new TimelineController({
        doc: docStore,
        view: viewStore,
        engine: engineStore,
        toasts: toastStore,
        t: (key, vars) => translate.current(key, vars),
        // Ruler scrubbing goes to the engine as latest-wins `view.seek` (WP-1M.04).
        seek: (at) => void transportController().seek(at),
        // A detached Timeline's drags pace on its own window's frames (WP-2H.15).
        frames: framesOf(() => surface.current),
      }),
    [],
  );

  // Commands outside the panel (the palette's Split at playhead) reach it through this.
  useEffect(() => setActiveTimelineController(controller), [controller]);

  // Losing the window mid-drag cancels the gesture (the pointer-up may never arrive): any editor
  // window, since the Timeline may be in a detached one.
  useEffect(() => {
    const blur = (): void => controller.cancel();
    const off = forEveryWindow((win) => {
      win.addEventListener("blur", blur);
      return () => win.removeEventListener("blur", blur);
    });
    return () => {
      off();
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
      // The right button and a long-press are the menu host's, not a gesture (WP-2H.12).
      if (e.type === "contextmenu" || e.type === "longpress") {
        const request = requestForCanvasEvent(e);
        if (request === null) closeContextMenu();
        else openContextMenu(request);
        return;
      }
      // Keys go where the pointer acted: focus the canvas on a press.
      if (e.type === "down") (e.original.target as HTMLElement | null)?.focus?.();
      controller.handlePointer(e);
    },
    [controller],
  );

  const view = useRef<TimelineView | null>(null);
  const onView = useCallback(
    (next: TimelineView | null) => {
      view.current = next;
      controller.attach(next);
    },
    [controller],
  );

  const canvasSurface = useCallback((el: HTMLElement | null) => {
    if (el === null) return undefined;
    surface.current = el;
    const off = registerTimelineCanvas(el, () => view.current);
    return () => {
      off();
      if (surface.current === el) surface.current = null;
    };
  }, []);

  const keyTarget = useCallback(
    (el: HTMLElement | null) => {
      if (el === null) return undefined;
      const refused = (edit: TimelineEdit, error: unknown): void => {
        const tr = translator(t);
        toastStore.getState().push({
          tone: "warning",
          title: tr(FAILED_TITLE[edit]),
          message: describeRefusal(toRpcError(error), tr),
        });
      };
      const run = (edit: TimelineEdit, work: () => Promise<void>): void => {
        void work().catch((error: unknown) => refused(edit, error));
      };
      // Copy, Cut and Paste come as clipboard events, from the keys and from the Edit menu alike.
      const onCopy = (e: ClipboardEvent): void => {
        if (isWritingClipboard() || isTextEntry(e.target)) return;
        const ids = selectedClips();
        if (ids.length === 0) return;
        e.preventDefault();
        run("copy", async () => void (await copyClips(engineCall, ids)));
      };
      const onCut = (e: ClipboardEvent): void => {
        if (isTextEntry(e.target)) return;
        const ids = selectedClips();
        if (ids.length === 0) return;
        e.preventDefault();
        run("cut", () => cutClips(engineCall, ids));
      };
      const onPaste = (e: ClipboardEvent): void => {
        if (isTextEntry(e.target)) return;
        const system = parsePayload(e.clipboardData?.getData(CLIPBOARD_MIME) ?? "");
        const payload = system ?? clipboardStore.getState().payload;
        if (payload === null) return;
        e.preventDefault();
        clipboardStore.getState().setPayload(payload);
        const view = viewStore.getState();
        const track = view.selection.find((r) => r.kind === "track")?.id;
        run("paste", () => pasteClips(engineCall, payload, view.playhead, track));
      };
      const onKey = (e: KeyboardEvent): void => {
        if (e.defaultPrevented || isTextEntry(e.target)) return;
        if (timelineEditorCommandOf(e) === "item.duplicate") {
          const ids = selectedClips();
          if (ids.length === 0) return;
          e.preventDefault();
          run("duplicate", () => duplicateClips(engineCall, ids));
          return;
        }
        // The keys are the keymap's `timeline` context (Settings > Keyboard shortcuts).
        const action = timelineActionOf(e);
        const handled = controller.handleKey({
          key: e.key,
          shiftKey: e.shiftKey,
          altKey: e.altKey,
          ctrlKey: e.ctrlKey,
          metaKey: e.metaKey,
          action,
        });
        if (handled) e.preventDefault();
      };
      el.addEventListener("keydown", onKey);
      el.addEventListener("copy", onCopy);
      el.addEventListener("cut", onCut);
      el.addEventListener("paste", onPaste);
      return () => {
        el.removeEventListener("keydown", onKey);
        el.removeEventListener("copy", onCopy);
        el.removeEventListener("cut", onCut);
        el.removeEventListener("paste", onPaste);
      };
    },
    [controller, t],
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

  return { controller, overlay, onPointer, onView, keyTarget, canvasSurface, drop };
}
