// SPDX-License-Identifier: GPL-3.0-or-later
// `useTimelineHost()` (WP-2A.10): one `TimelineHost` per timeline panel, bound to the app's stores,
// with the callbacks the panel hands to `TimelineCanvas` and the elements around it: pointer
// events, keys (keydown and keyup, so held modifiers reach the machine), the clipboard events,
// a bin card dragged over the lanes, and a lost window.
import {
  FLICKS_PER_SECOND,
  type TimelinePointerEvent,
  type TimelineView,
} from "@cutweave/timeline";
import { type DragEvent, useCallback, useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { engineStore } from "../engine/engineStore.ts";
import { timelineKeyOf } from "../keymap/timeline.ts";
import { dragStore } from "../media/dragStore.ts";
import { requestForCanvasEvent } from "../menus/canvas.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { closeContextMenu, openContextMenu } from "../menus/host.tsx";
import { registerTimelineCanvas } from "../menus/timelineSurface.ts";
import { prefsStore } from "../prefs/prefsStore.ts";
import { isHTMLElement } from "../shell/realm.ts";
import { forEveryWindow, framesOf } from "../shell/windows.ts";
import { matchFrame } from "../source/actions.ts";
import { docStore } from "../state/docStore.ts";
import { selectTimelineModel } from "../state/selectors.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { transportController } from "../transport/useTransport.ts";
import { setActiveTimelineHost } from "./activeHost.ts";
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
import { describeRefusal, FAILED_TITLE, type TimelineEdit, translator } from "./errors.ts";
import { toRpcError } from "./gestureTx.ts";
import { type LaneBox, TimelineHost, type TimelineUi, withCode } from "./host.ts";
import {
  copyKeys,
  deleteKeys,
  isWritingKeys,
  KEY_CLIPBOARD_MIME,
  keyClipboardStore,
  keySelectionStore,
  parseKeyPayload,
  pasteKeys,
} from "./keyframes.ts";
import { locksStore } from "./locks.ts";
import { MarkerDrag, onRuler } from "./markerDrag.ts";
import { machinePrefs, newTrackHeight } from "./prefs.ts";

export interface TimelineBinding {
  readonly host: TimelineHost;
  /** Tool, guides, marquee, drop ghost and cursor. */
  readonly ui: TimelineUi;
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
  /** Ref for the panel root: the timeline's keys and the clipboard events inside it. */
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

/** A CSS length token in px, read where the canvas is (the ruler is `control-md` tall). */
function tokenPx(el: Element, name: string): number {
  const view = el.ownerDocument.defaultView;
  if (view === null) return 0;
  return Number.parseFloat(view.getComputedStyle(el).getPropertyValue(name)) || 0;
}

/** The lane area of the canvas: right of the headers, under the ruler. */
function laneBox(view: TimelineView, canvas: HTMLElement): () => LaneBox {
  let ruler = 0;
  // The ruler is taller in the Comfortable density (WP-2H.09): read again when it changes.
  let density: string | null = null;
  return () => {
    const now = canvas.ownerDocument.documentElement.getAttribute("data-density");
    if (ruler === 0 || now !== density) {
      ruler = tokenPx(canvas, "--control-md");
      density = now;
    }
    return {
      left: view.timeToX(view.getViewState().scrollX),
      right: canvas.clientWidth,
      top: ruler,
      bottom: canvas.clientHeight,
    };
  };
}

export function useTimelineHost(): TimelineBinding {
  const { t } = useTranslation();
  const translate = useRef(translator(t));
  translate.current = translator(t);
  /** The canvas's surface element: its window is where the gesture's frames come from. */
  const surface = useRef<HTMLElement | null>(null);

  const host = useMemo(
    () =>
      new TimelineHost({
        doc: docStore,
        view: viewStore,
        engine: engineStore,
        toasts: toastStore,
        locks: locksStore,
        t: (key, vars) => translate.current(key, vars),
        // Ruler scrubbing goes to the engine as latest-wins `view.seek` (WP-1M.04).
        seek: (at) => void transportController().seek(at),
        // A detached Timeline's drags pace on its own window's frames (WP-2H.15).
        frames: framesOf(() => surface.current),
        // Settings > Editing and Settings > Timeline (WP-2A.13).
        prefs: () => machinePrefs(prefsStore.getState().values),
        linkAv: () => prefsStore.getState().values["editing.link_av"],
        matchFrame: () => void matchFrame(),
        newTrackHeight: () =>
          newTrackHeight(prefsStore.getState().values["timeline.track_height"], surface.current),
      }),
    [],
  );

  const view = useRef<TimelineView | null>(null);

  // A marker dragged along the ruler (issue #222): its own gesture, beside the machine's.
  const markerDrag = useMemo(
    () =>
      new MarkerDrag({
        rpc: () => engineStore.getState().rpc,
        label: () => translate.current("timeline.history.marker"),
        markerAt: (x, y) => view.current?.markerAt(x, y) ?? null,
        model: () => selectTimelineModel(docStore.getState().project),
        flicksPerPx: () => FLICKS_PER_SECOND / (view.current?.getViewState().pxPerSec ?? 100),
        snap: (model, raw, suspended) => host.snapTime(model, raw, suspended),
        guides: (snapGuides) => host.ui.setState({ snapGuides }),
        seek: (t) => void transportController().seek(t),
        refused: (error) =>
          toastStore.getState().push({
            tone: "warning",
            title: translate.current("timeline.failed.marker"),
            message: withCode(describeRefusal(error, translate.current), error.code),
          }),
        busy: () => host.busy,
        frames: framesOf(() => surface.current),
      }),
    [host],
  );

  // Commands outside the panel (the palette's Split at playhead) reach it through this.
  useEffect(() => setActiveTimelineHost(host), [host]);

  // Losing the window mid-drag cancels the gesture (the pointer-up may never arrive): any editor
  // window, since the Timeline may be in a detached one.
  // The host follows the stores while the panel is mounted; unmounting rolls back a gesture.
  useEffect(() => {
    host.start();
    const blur = (): void => {
      markerDrag.cancel();
      host.cancel();
    };
    // A change in Settings reaches the machine at once.
    const offPrefs = prefsStore.subscribe((state, previous) => {
      if (state.values !== previous.values) host.syncPrefs();
    });
    const off = forEveryWindow((win) => {
      win.addEventListener("blur", blur);
      return () => win.removeEventListener("blur", blur);
    });
    return () => {
      off();
      offPrefs();
      markerDrag.cancel();
      host.stop();
    };
  }, [host, markerDrag]);

  // A card drag that ends without a drop here (Esc, dropped elsewhere) rolls its placement back.
  useEffect(
    () =>
      dragStore.subscribe((state) => {
        if (state.session === null) host.mediaLeave();
      }),
    [host],
  );

  const ui = useStore(host.ui);

  const onPointer = useCallback(
    (e: TimelinePointerEvent) => {
      // The right button and a long-press are the menu host's, not a gesture (WP-2H.12).
      if (e.type === "contextmenu" || e.type === "longpress") {
        const marker = e.hit.zone === "ruler" ? (view.current?.markerAt(e.x, e.y) ?? null) : null;
        const original = e.original;
        if (marker !== null) {
          openContextMenu({
            surface: "timeline.marker",
            point: { x: original.clientX, y: original.clientY },
            via: e.type === "longpress" ? "touch" : "pointer",
            element: isHTMLElement(original.target) ? original.target : null,
            target: { ids: [marker.id], name: marker.label, place: { time: marker.t } },
          });
          return;
        }
        // A right-click on a key acts on it: it joins (or becomes) the key selection first.
        host.contextKey(e.hit);
        const request = requestForCanvasEvent(e);
        if (request === null) closeContextMenu();
        else openContextMenu(request);
        return;
      }
      // Keys go where the pointer acted: focus the canvas on a press.
      if (e.type === "down") (e.original.target as HTMLElement | null)?.focus?.();
      // A double-click on a marker opens its Edit… dialog (issue #222).
      if (e.type === "dblclick" && onRuler(e)) {
        const marker = view.current?.markerAt(e.x, e.y) ?? null;
        if (marker !== null) {
          markerDrag.cancel();
          menuDialogStore.getState().open({ kind: "edit-marker", marker: marker.id });
          return;
        }
      }
      // A press on a marker's pin is the marker's: a click goes to it, a drag moves it.
      if (markerDrag.handle(e)) return;
      host.handlePointer(e);
    },
    [host, markerDrag],
  );

  const onView = useCallback(
    (next: TimelineView | null) => {
      view.current = next;
      const canvas = surface.current?.querySelector("canvas") ?? null;
      host.attach(next, canvas, next !== null && canvas !== null ? laneBox(next, canvas) : null);
    },
    [host],
  );

  const canvasSurface = useCallback(
    (el: HTMLElement | null) => {
      if (el === null) return undefined;
      surface.current = el;
      const canvas = el.querySelector("canvas");
      if (view.current !== null && canvas !== null) {
        host.attach(view.current, canvas, laneBox(view.current, canvas));
      }
      const off = registerTimelineCanvas(el, () => view.current);
      return () => {
        off();
        if (surface.current === el) surface.current = null;
      };
    },
    [host],
  );

  const keyTarget = useCallback(
    (el: HTMLElement | null) => {
      if (el === null) return undefined;
      const refused = (edit: TimelineEdit, error: unknown): void => {
        const tr = translator(t);
        const refusal = toRpcError(error);
        toastStore.getState().push({
          tone: "warning",
          title: tr(FAILED_TITLE[edit]),
          message: withCode(describeRefusal(refusal, tr), refusal.code),
        });
      };
      const run = (edit: TimelineEdit, work: () => Promise<void>): void => {
        void work().catch((error: unknown) => refused(edit, error));
      };
      // Copy, Cut and Paste come as clipboard events, from the keys and from the Edit menu alike.
      /** A keyframe edit from the clipboard keys; a refusal is a toast. */
      const runKeys = (
        title: "timeline.failed.keyframeCopy" | "timeline.failed.keyframePaste",
        work: () => Promise<void>,
      ) => {
        void work().catch((error: unknown) => {
          const tr = translator(t);
          const refusal = toRpcError(error);
          toastStore.getState().push({
            tone: "warning",
            title: tr(title),
            message: withCode(describeRefusal(refusal, tr), refusal.code),
          });
        });
      };
      const onCopy = (e: ClipboardEvent): void => {
        if (isWritingClipboard() || isWritingKeys() || isTextEntry(e.target)) return;
        // Selected keys are what Copy copies (WP-2C.05); else the selected clips.
        const keys = keySelectionStore.getState();
        if (keys.item !== null && keys.keys.length > 0) {
          const item = keys.item;
          e.preventDefault();
          runKeys(
            "timeline.failed.keyframeCopy",
            async () => void (await copyKeys(engineCall, item, keys.keys)),
          );
          return;
        }
        const ids = selectedClips();
        if (ids.length === 0) return;
        e.preventDefault();
        keyClipboardStore.setState({ payload: null });
        run("copy", async () => void (await copyClips(engineCall, ids)));
      };
      const onCut = (e: ClipboardEvent): void => {
        if (isTextEntry(e.target)) return;
        // Selected keys are what Cut takes (never their clip): copied, then deleted in one step.
        const keys = keySelectionStore.getState();
        if (keys.item !== null && keys.keys.length > 0) {
          const item = keys.item;
          e.preventDefault();
          runKeys("timeline.failed.keyframeCopy", async () => {
            await copyKeys(engineCall, item, keys.keys);
            await deleteKeys(engineCall, item, keys.keys);
          });
          return;
        }
        const ids = selectedClips();
        if (ids.length === 0) return;
        e.preventDefault();
        run("cut", () => cutClips(engineCall, ids));
      };
      const onPaste = (e: ClipboardEvent): void => {
        if (isTextEntry(e.target)) return;
        // Copied keys go onto the clip whose keys are selected, else the one selected clip, with
        // the earliest at the playhead (WP-2C.05).
        const text = e.clipboardData?.getData(KEY_CLIPBOARD_MIME) ?? "";
        const clips = e.clipboardData?.getData(CLIPBOARD_MIME) ?? "";
        const copied =
          parseKeyPayload(text) ?? (clips === "" ? keyClipboardStore.getState().payload : null);
        if (copied !== null) {
          const onto = host.keyPasteTarget();
          if (onto !== null) {
            e.preventDefault();
            const at = viewStore.getState().playhead;
            runKeys("timeline.failed.keyframePaste", () => pasteKeys(engineCall, copied, onto, at));
            return;
          }
        }
        const system = parsePayload(e.clipboardData?.getData(CLIPBOARD_MIME) ?? "");
        const payload = system ?? clipboardStore.getState().payload;
        if (payload === null) return;
        e.preventDefault();
        clipboardStore.getState().setPayload(payload);
        const v = viewStore.getState();
        const track = v.selection.find((r) => r.kind === "track")?.id;
        run("paste", () => pasteClips(engineCall, payload, v.playhead, track));
      };
      const onKeyDown = (e: KeyboardEvent): void => {
        if (e.defaultPrevented || isTextEntry(e.target)) return;
        // Esc while a marker is dragged puts it back.
        if (e.key === "Escape" && markerDrag.cancel()) {
          e.preventDefault();
          return;
        }
        // The keys are the keymap's `timeline` context (Settings > Keyboard shortcuts).
        const key = timelineKeyOf(e);
        if (key.kind === "editor") {
          if (key.command === "item.duplicate") {
            const ids = selectedClips();
            if (ids.length === 0) return;
            e.preventDefault();
            run("duplicate", () => duplicateClips(engineCall, ids));
            return;
          }
          if (host.runHostKey(key.command)) e.preventDefault();
          return;
        }
        const action = key.kind === "action" ? key.action : null;
        if (host.handleKey(e, true, action)) e.preventDefault();
      };
      const onKeyUp = (e: KeyboardEvent): void => {
        if (isTextEntry(e.target)) return;
        host.handleKey(e, false);
      };
      el.addEventListener("keydown", onKeyDown);
      el.addEventListener("keyup", onKeyUp);
      el.addEventListener("copy", onCopy);
      el.addEventListener("cut", onCut);
      el.addEventListener("paste", onPaste);
      return () => {
        el.removeEventListener("keydown", onKeyDown);
        el.removeEventListener("keyup", onKeyUp);
        el.removeEventListener("copy", onCopy);
        el.removeEventListener("cut", onCut);
        el.removeEventListener("paste", onPaste);
      };
    },
    [host, markerDrag, t],
  );

  const drop = useMemo<TimelineBinding["drop"]>(() => {
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
        const lands = host.mediaOver(at.x, at.y, media, { alt: e.altKey });
        // Allowing the drop is what shows the "copy" cursor; with nowhere to land it stays "no drop".
        if (lands) {
          e.preventDefault();
          e.dataTransfer.dropEffect = "copy";
        }
      },
      onDragLeave(e) {
        // Moving between children fires leave too: only a real exit rolls the placement back.
        const next = e.relatedTarget as Node | null;
        if (next === null || !e.currentTarget.contains(next)) host.mediaLeave();
      },
      onDrop(e) {
        const media = dragStore.getState().session;
        const at = media === null ? null : local(e);
        if (media === null || at === null) return;
        e.preventDefault();
        host.mediaDrop(at.x, at.y, media, { alt: e.altKey });
        dragStore.getState().end();
      },
    };
  }, [host]);

  return { host, ui, onPointer, onView, keyTarget, canvasSurface, drop };
}
