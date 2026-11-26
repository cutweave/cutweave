// SPDX-License-Identifier: GPL-3.0-or-later
// The preview's transform handles (WP-2H.06, arch §16): over the picture, the selected clip's box
// with scale handles (a corner keeps the shape, Shift frees it), a rotate handle and the anchor
// point; crop mode swaps the scale handles for crop handles (Shift crops the opposite edge too);
// several clips show one bounding box that moves them together. Drags snap to the canvas edges,
// halves and quarters and to the other pictures' edges (the timeline's snapping switch), and every
// drag is one `gesture` transaction of `item.set_props`, so it is one undo step and the picture
// follows live. Arrow keys nudge by one sequence pixel, ten with Shift.
//
// It is DOM in the preview frame (same page as the canvas, so it lines up at every size) and, unlike
// the canvas, may draw outside it: a clip scaled past the frame keeps its handles reachable.

import { useOwnerWindow } from "@cutweave/ui";
import {
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { engineStore } from "../../engine/engineStore.ts";
import { i18n } from "../../i18n/index.ts";
import { matchEvent } from "../../keymap/store.ts";
import { observeSize } from "../../shell/windows.ts";
import { useDocStore } from "../../state/docStore.ts";
import { toastStore } from "../../state/toastStore.ts";
import { useViewStore, viewStore } from "../../state/viewStore.ts";
import { translator } from "../../timeline/errors.ts";
import { clientToCanvas, type LayerState, type Pt, type Size, viewScale } from "./geometry.ts";
import { guideStore, useGuides } from "./guides.ts";
import { OverlayEditor, type PropsPatch, setPropsStep } from "./overlayEditor.ts";
import { buildScene, type Scene, type SceneLayer, unionBounds } from "./scene.ts";
import {
  type Guide,
  guideSegment,
  SNAP_DISTANCE_PX,
  type SnapTargets,
  snapBounds,
  snapPoint,
  targetsWith,
} from "./snapping.ts";
import {
  anchorDrag,
  cropDrag,
  handleCanvasPoint,
  type ItemEdge,
  moveBy,
  rotateDrag,
  type ScaleHandle,
  scaleDrag,
} from "./transformOps.ts";

const HANDLES: readonly ScaleHandle[] = ["tl", "t", "tr", "r", "br", "b", "bl", "l"];
const CROP_EDGES: Readonly<Record<ScaleHandle, readonly ItemEdge[]>> = {
  tl: ["left", "top"],
  t: ["top"],
  tr: ["right", "top"],
  r: ["right"],
  br: ["right", "bottom"],
  b: ["bottom"],
  bl: ["left", "bottom"],
  l: ["left"],
};
/** How far the rotate handle stands off the top edge, in screen pixels. */
const ROTATE_OFFSET_PX = 28;

type DragKind =
  | { readonly kind: "move" }
  | { readonly kind: "scale"; readonly handle: ScaleHandle }
  | { readonly kind: "rotate" }
  | { readonly kind: "anchor" }
  | { readonly kind: "crop"; readonly handle: ScaleHandle };

interface ActiveDrag {
  readonly what: DragKind;
  readonly layers: readonly SceneLayer[];
  readonly startPointer: Pt;
  /** Where on its handle the pointer took hold, so the handle does not jump to the pointer. */
  readonly grab: Pt;
  readonly targets: SnapTargets;
  readonly canvas: Size;
  readonly playhead: number;
  readonly label: string;
}

const pct = (v: number): string => `${(v * 100).toFixed(4)}%`;

function useOverlayEditor(): OverlayEditor {
  const editor = useMemo(
    () =>
      new OverlayEditor({
        engine: engineStore,
        toasts: toastStore,
        t: (key, vars) => translator(i18n.t.bind(i18n))(key, vars),
      }),
    [],
  );
  useEffect(
    () => () => {
      void editor.dispose();
    },
    [editor],
  );
  return editor;
}

/** The overlay's width on screen, for sizes that stay constant in screen pixels. */
function useWidth(ref: React.RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  // Observed from its own window (a detached Preview's), again when it moves.
  const ownerWindow = useOwnerWindow();
  useEffect(() => {
    const el = ref.current;
    if (el === null || ownerWindow === null) return undefined;
    const read = () => setWidth(el.getBoundingClientRect().width);
    read();
    return observeSize(el, read);
  }, [ref, ownerWindow]);
  return width;
}

function handlePoint(layer: SceneLayer, handle: ScaleHandle): Pt {
  return handleCanvasPoint(layer.geom, handle);
}

export function TransformOverlay() {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const selection = useViewStore((s) => s.selection);
  const playhead = useViewStore((s) => s.playhead);
  const cropMode = useGuides((s) => s.cropMode);
  const scene = useMemo(
    () => buildScene(project, selection, playhead),
    [project, selection, playhead],
  );
  const rootRef = useRef<HTMLDivElement>(null);
  const editor = useOverlayEditor();
  const drag = useRef<ActiveDrag | null>(null);
  const [guides, setGuides] = useState<readonly Guide[]>([]);
  const width = useWidth(rootRef);
  const hintId = useId();
  const latest = useRef<Scene | null>(scene);
  latest.current = scene;

  // The selection moved on (or the playhead left the clip): an open drag rolls back.
  const selectedKey = scene?.selected.map((l) => l.id).join(",") ?? "";
  useEffect(() => {
    return () => {
      drag.current = null;
      void editor.cancel();
    };
  }, [selectedKey, editor]);

  const zoom = (() => {
    const canvas = scene?.canvas;
    return canvas === undefined || width <= 0 ? 1 : width / canvas.w;
  })();

  const begin = (e: PointerEvent<HTMLElement | SVGElement>, what: DragKind): void => {
    const current = latest.current;
    const root = rootRef.current;
    if (e.button !== 0 || current === null || root === null) return;
    const layers = current.selected.filter((l) => !l.locked);
    if (layers.length === 0) return;
    e.preventDefault();
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    root.focus({ preventScroll: true });
    const rect = root.getBoundingClientRect();
    const view = {
      left: rect.left,
      top: rect.top,
      width: rect.width,
      height: rect.height,
      canvas: current.canvas,
    };
    const labels = {
      move: "preview.overlay.history.move",
      scale: "preview.overlay.history.scale",
      rotate: "preview.overlay.history.rotate",
      anchor: "preview.overlay.history.anchor",
      crop: "preview.overlay.history.crop",
    } as const;
    const startPointer = clientToCanvas(view, e.clientX, e.clientY);
    const first = layers[0];
    const held =
      first !== undefined && (what.kind === "scale" || what.kind === "crop")
        ? handleCanvasPoint(first.geom, what.handle)
        : startPointer;
    drag.current = {
      what,
      layers,
      startPointer,
      grab: { x: startPointer.x - held.x, y: startPointer.y - held.y },
      targets: targetsWith(current.canvas, current.others),
      canvas: current.canvas,
      playhead: current.playhead,
      label: t(labels[what.kind]),
    };
  };

  const move = (e: PointerEvent<HTMLElement | SVGElement>): void => {
    const d = drag.current;
    const root = rootRef.current;
    if (d === null || root === null) return;
    const rect = root.getBoundingClientRect();
    const view = {
      left: rect.left,
      top: rect.top,
      width: rect.width,
      height: rect.height,
      canvas: d.canvas,
    };
    const pointer = clientToCanvas(view, e.clientX, e.clientY);
    const within = SNAP_DISTANCE_PX / Math.max(viewScale(view), 1e-6);
    const snapping = viewStore.getState().snapping;
    const first = d.layers[0];
    if (first === undefined) return;
    let found: readonly Guide[] = [];
    let patches: { layer: SceneLayer; patch: PropsPatch }[] = [];
    const axisAligned = (l: LayerState) => Math.abs(l.rotation % 90) < 1e-9;

    switch (d.what.kind) {
      case "move": {
        let delta = { x: pointer.x - d.startPointer.x, y: pointer.y - d.startPointer.y };
        if (e.shiftKey)
          delta =
            Math.abs(delta.x) >= Math.abs(delta.y) ? { x: delta.x, y: 0 } : { x: 0, y: delta.y };
        const box = unionBounds(d.layers);
        if (snapping && box !== null) {
          const snap = snapBounds(
            [box[0] + delta.x, box[1] + delta.y, box[2] + delta.x, box[3] + delta.y],
            d.targets,
            within,
          );
          // A locked axis (Shift) does not snap across.
          delta = {
            x: e.shiftKey && delta.x === 0 ? 0 : delta.x + snap.dx,
            y: e.shiftKey && delta.y === 0 ? 0 : delta.y + snap.dy,
          };
          found = snap.guides;
        }
        patches = d.layers.map((layer) => ({
          layer,
          patch: { transform: { position: moveBy(layer.state, delta, d.canvas) } },
        }));
        break;
      }
      case "scale": {
        let p = { x: pointer.x - d.grab.x, y: pointer.y - d.grab.y };
        if (snapping && axisAligned(first.state)) {
          const snap = snapPoint(p, d.targets, within);
          p = snap.point;
          found = snap.guides;
        }
        const out = scaleDrag(first.state, first.geom, d.canvas, d.what.handle, p, e.shiftKey);
        patches = [
          { layer: first, patch: { transform: { scale: out.scale, position: out.position } } },
        ];
        break;
      }
      case "rotate": {
        const rotation = rotateDrag(first.state, first.geom, d.startPointer, pointer, e.shiftKey);
        patches = [{ layer: first, patch: { transform: { rotation } } }];
        break;
      }
      case "anchor": {
        const out = anchorDrag(first.geom, d.canvas, pointer);
        patches = [
          { layer: first, patch: { transform: { anchor: out.anchor, position: out.position } } },
        ];
        break;
      }
      case "crop": {
        let p = { x: pointer.x - d.grab.x, y: pointer.y - d.grab.y };
        if (snapping && axisAligned(first.state)) {
          const snap = snapPoint(p, d.targets, within);
          p = snap.point;
          found = snap.guides;
        }
        const crop = cropDrag(first.state, first.geom, CROP_EDGES[d.what.handle], p, e.shiftKey);
        patches = [{ layer: first, patch: { crop } }];
        break;
      }
    }
    setGuides(found);
    editor.live(
      d.label,
      patches.map(({ layer, patch }) => setPropsStep(layer.item.id, patch, d.playhead)),
    );
  };

  const end = (e: PointerEvent<HTMLElement | SVGElement>): void => {
    if (drag.current === null) return;
    drag.current = null;
    setGuides([]);
    if (e.currentTarget.hasPointerCapture(e.pointerId))
      e.currentTarget.releasePointerCapture(e.pointerId);
    void editor.commit();
  };

  const abort = (): void => {
    if (drag.current === null) return;
    drag.current = null;
    setGuides([]);
    void editor.cancel();
  };

  const dragProps = (what: DragKind) => ({
    onPointerDown: (e: PointerEvent<HTMLElement | SVGElement>) => begin(e, what),
    onPointerMove: move,
    onPointerUp: end,
    onPointerCancel: abort,
  });

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>): void => {
    const current = latest.current;
    if (current === null || e.defaultPrevented) return;
    if (e.key === "Escape") {
      if (drag.current !== null) {
        e.preventDefault();
        e.stopPropagation();
        abort();
      } else if (guideStore.getState().cropMode) {
        e.preventDefault();
        guideStore.getState().set({ cropMode: false });
      }
      return;
    }
    let binding = matchEvent(e, ["preview", "global"]);
    // Shift is the big step: a plain key ignores it, like the bin's.
    if (binding === null && e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey) {
      binding = matchEvent(
        {
          key: e.key,
          code: e.code,
          ctrlKey: false,
          metaKey: false,
          altKey: false,
          shiftKey: false,
        },
        ["preview", "global"],
      );
    }
    if (binding === null) return;
    if (binding.command === "preview.toggle_crop") {
      e.preventDefault();
      guideStore.getState().set({ cropMode: !guideStore.getState().cropMode });
      return;
    }
    const step = e.shiftKey ? 10 : 1;
    const delta: Pt | null =
      binding.command === "preview.nudge_left"
        ? { x: -step, y: 0 }
        : binding.command === "preview.nudge_right"
          ? { x: step, y: 0 }
          : binding.command === "preview.nudge_up"
            ? { x: 0, y: -step }
            : binding.command === "preview.nudge_down"
              ? { x: 0, y: step }
              : null;
    if (delta === null) return;
    e.preventDefault();
    e.stopPropagation();
    const layers = current.selected.filter((l) => !l.locked);
    void editor.apply(
      t("preview.overlay.history.nudge"),
      layers.map((l) =>
        setPropsStep(
          l.item.id,
          { transform: { position: moveBy(l.state, delta, current.canvas) } },
          current.playhead,
        ),
      ),
    );
  };

  if (scene === null || scene.selected.length === 0) return null;
  const { canvas, selected } = scene;
  const single = selected.length === 1 ? selected[0] : undefined;
  const box = unionBounds(selected);
  const name = single?.item.name ?? "";
  const toLocal = (p: Pt): CSSProperties => ({
    left: pct(p.x / canvas.w),
    top: pct(p.y / canvas.h),
  });
  const polygon = (l: SceneLayer) => l.geom.corners.map((p) => `${p.x},${p.y}`).join(" ");

  return (
    <div
      ref={rootRef}
      className="cw-xform"
      // The box is the keyboard's way in: arrows nudge, C crops, Escape ends a drag or crop mode.
      role="application"
      // biome-ignore lint/a11y/noNoninteractiveTabindex: a keyboard widget (arrows nudge, C crops): it has the focus so those keys reach it
      tabIndex={0}
      aria-label={
        single === undefined
          ? t("preview.overlay.handlesMany", { count: selected.length })
          : t("preview.overlay.handles", { name })
      }
      aria-describedby={hintId}
      onKeyDown={onKeyDown}
      data-mode={cropMode && single !== undefined ? "crop" : "transform"}
    >
      <span id={hintId} className="cw-xform__hint">
        {t("preview.overlay.boxHint")}
      </span>
      <svg
        className="cw-xform__svg"
        viewBox={`0 0 ${canvas.w} ${canvas.h}`}
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        {selected.map((l) => (
          <polygon key={l.id} className="cw-xform__outline" points={polygon(l)} />
        ))}
        {single === undefined && box !== null ? (
          <rect
            className="cw-xform__bounds"
            x={box[0]}
            y={box[1]}
            width={box[2] - box[0]}
            height={box[3] - box[1]}
            {...dragProps({ kind: "move" })}
          />
        ) : null}
        {single === undefined ? null : (
          <polygon
            className="cw-xform__body"
            data-locked={single.locked || undefined}
            points={polygon(single)}
            {...dragProps({ kind: "move" })}
          />
        )}
        {guides.map((g) => (
          <line key={`${g.axis}${g.at}`} className="cw-xform__guide" {...guideSegment(g, canvas)} />
        ))}
        {single !== undefined && !cropMode ? <RotateStalk layer={single} zoom={zoom} /> : null}
      </svg>
      {single === undefined || single.locked ? null : (
        <>
          {HANDLES.map((h) => (
            <span
              key={h}
              className="cw-xform__handle"
              data-handle={h}
              data-kind={cropMode ? "crop" : "scale"}
              data-edge={h.length === 1 ? "" : undefined}
              aria-hidden="true"
              style={
                {
                  ...toLocal(handlePoint(single, h)),
                  "--cw-xform-turn": `${single.state.rotation}deg`,
                } as CSSProperties
              }
              {...dragProps(cropMode ? { kind: "crop", handle: h } : { kind: "scale", handle: h })}
            />
          ))}
          {cropMode ? null : (
            <>
              <span
                className="cw-xform__handle"
                data-handle="rotate"
                data-kind="rotate"
                aria-hidden="true"
                style={toLocal(rotatePoint(single, zoom))}
                {...dragProps({ kind: "rotate" })}
              />
              <span
                className="cw-xform__anchor"
                data-handle="anchor"
                aria-hidden="true"
                style={toLocal(single.geom.anchorAt)}
                {...dragProps({ kind: "anchor" })}
              />
            </>
          )}
        </>
      )}
    </div>
  );
}

/** Where the rotate handle stands: out from the top edge's middle, away from the box's centre. */
function rotatePoint(layer: SceneLayer, zoom: number): Pt {
  const top = handlePoint(layer, "t");
  const bottom = handlePoint(layer, "b");
  const dx = top.x - bottom.x;
  const dy = top.y - bottom.y;
  const len = Math.hypot(dx, dy) || 1;
  const away = ROTATE_OFFSET_PX / Math.max(zoom, 1e-6);
  return { x: top.x + (dx / len) * away, y: top.y + (dy / len) * away };
}

function RotateStalk({ layer, zoom }: { layer: SceneLayer; zoom: number }) {
  if (layer.locked) return null;
  const from = handlePoint(layer, "t");
  const to = rotatePoint(layer, zoom);
  return <line className="cw-xform__stalk" x1={from.x} y1={from.y} x2={to.x} y2={to.y} />;
}
