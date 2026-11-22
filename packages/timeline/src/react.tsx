// SPDX-License-Identifier: GPL-3.0-or-later
// React wrapper for `TimelineView`. Thin on purpose: it owns the canvas element, creates and
// destroys the view, and forwards props. All timeline logic stays in the framework-agnostic class.
import { type CSSProperties, useEffect, useRef } from "react";
import type { MediaLoaders } from "./cache.ts";
import type { TimelineModel, ViewState } from "./model.ts";
import { type TimelinePointerEvent, TimelineView } from "./timeline-view.ts";

export interface TimelineCanvasProps {
  readonly model: TimelineModel;
  /** Controlled view state (zoom, scroll, playhead, selection, ...). Applied whenever it changes. */
  readonly viewState?: Partial<ViewState>;
  /** Pluggable media loaders (filmstrip tiles, waveform peaks). */
  readonly loaders?: MediaLoaders;
  readonly headerWidth?: number;
  /** Pointer events with their hit-test result (consumed by the interaction layer, WP-1M.06). */
  readonly onPointer?: (e: TimelinePointerEvent) => void;
  /** Zoom and scroll caused by the user (wheel, drag, `+`/`-`), for the host's `viewStore`. */
  readonly onViewChange?: (s: ViewState) => void;
  /** Receives the live view (null on unmount) for imperative calls such as `zoomToFit()`. */
  readonly onView?: (view: TimelineView | null) => void;
  readonly "aria-label": string;
  readonly className?: string;
  readonly style?: CSSProperties;
}

const FILL: CSSProperties = {
  position: "relative",
  width: "100%",
  height: "100%",
  overflow: "hidden",
};
const CANVAS: CSSProperties = { position: "absolute", inset: 0, display: "block", outline: "none" };

export function TimelineCanvas(props: TimelineCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewRef = useRef<TimelineView | null>(null);
  // Latest props for the long-lived view's callbacks and the creation effect.
  const latest = useRef(props);
  latest.current = props;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const p = latest.current;
    const view = new TimelineView(canvas, {
      ...(p.loaders && { loaders: p.loaders }),
      ...(p.headerWidth !== undefined && { headerWidth: p.headerWidth }),
    });
    viewRef.current = view;
    view.bind();
    view.setModel(p.model);
    if (p.viewState) view.setViewState(p.viewState);
    const offPointer = view.onPointer((e) => latest.current.onPointer?.(e));
    const offView = view.onViewChange((s) => latest.current.onViewChange?.(s));
    p.onView?.(view);
    view.flush(); // first frame synchronously: no blank canvas, deterministic screenshots
    return () => {
      offPointer();
      offView();
      latest.current.onView?.(null);
      view.destroy();
      viewRef.current = null;
    };
  }, []);

  useEffect(() => {
    viewRef.current?.setModel(props.model);
  }, [props.model]);

  useEffect(() => {
    if (props.viewState) viewRef.current?.setViewState(props.viewState);
  }, [props.viewState]);

  useEffect(() => {
    if (props.loaders) viewRef.current?.setLoaders(props.loaders);
  }, [props.loaders]);

  return (
    <div
      className={props.className}
      style={{ ...FILL, ...props.style }}
      role="application"
      aria-label={props["aria-label"]}
    >
      <canvas ref={canvasRef} style={CANVAS} tabIndex={0} />
    </div>
  );
}
