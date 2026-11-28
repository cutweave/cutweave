// SPDX-License-Identifier: GPL-3.0-or-later
// The graph editor (WP-2C.07): value-over-time curves of animated properties on one canvas, with
// keys to select, box-select and drag in time and value, bezier handles, a time and a value
// ruler, the playhead, pan, zoom and fit. This component is the canvas's small React shell: the
// drawing and the input live in `GraphView` (view.ts), which the host can also drive directly
// (`onView`), so new curves, a moving playhead or a pan redraw the canvas and render nothing.
import { useEffect, useId, useLayoutEffect, useRef } from "react";
import { cx } from "../components/cx.ts";
import { useOwnerWindow } from "../components/PortalContainer.ts";
import type { GraphViewport, TangentMode } from "./geometry.ts";
import type { GraphTheme } from "./theme.ts";
import {
  type GraphCurve,
  type GraphEvents,
  type GraphOptions,
  type GraphPointRef,
  GraphView,
} from "./view.ts";

/** Every string the canvas has; override per locale. */
export interface GraphEditorLabels {
  /** The canvas's name. */
  graph: string;
  /** How to use it from the keyboard (read after the name). */
  help: string;
}

export const DEFAULT_GRAPH_LABELS: GraphEditorLabels = {
  graph: "Graph editor",
  help:
    "Alt+Left and Alt+Right select a key. The arrow keys move the selected keys in time and " +
    "value, Shift moves them further. Delete removes them, Enter opens the easing, F fits the " +
    "curves.",
};

/** What a host driving the canvas itself gets. */
export interface GraphEditorHandle {
  readonly view: GraphView;
  /** Says `text` to assistive technology (the selection in words). */
  announce(text: string): void;
}

export interface GraphEditorProps {
  options: GraphOptions;
  /**
   * The data, declaratively. A host that changes it often leaves these out and calls the view's
   * setters from `onView`, which renders nothing.
   */
  curves?: readonly GraphCurve[] | undefined;
  selection?: readonly GraphPointRef[] | undefined;
  playhead?: number | null | undefined;
  /** The window shown; `null` fits the curves. */
  viewport?: GraphViewport | null | undefined;
  normalized?: boolean | undefined;
  tangentMode?: TangentMode | undefined;
  events?: GraphEvents | undefined;
  /** The canvas is ready (`null`: it is going away). */
  onView?: ((handle: GraphEditorHandle | null) => void) | undefined;
  /** Fixed tokens instead of the document's (tests). */
  theme?: GraphTheme | undefined;
  labels?: Partial<GraphEditorLabels> | undefined;
  className?: string | undefined;
}

export function GraphEditor({
  options,
  curves,
  selection,
  playhead,
  viewport,
  normalized,
  tangentMode,
  events,
  onView,
  theme,
  labels: given,
  className,
}: GraphEditorProps) {
  const labels = { ...DEFAULT_GRAPH_LABELS, ...given };
  const helpId = useId();
  const canvas = useRef<HTMLCanvasElement>(null);
  const status = useRef<HTMLDivElement>(null);
  const view = useRef<GraphView | null>(null);
  const win = useOwnerWindow();
  const ready = useRef(onView);
  ready.current = onView;
  const first = useRef({ options, theme });

  // One view per canvas; it is bound again when the canvas moves to another window (a detached
  // panel), whose observers and frames it must use.
  useLayoutEffect(() => {
    const el = canvas.current;
    if (el === null) return;
    let v = view.current;
    if (v === null) {
      const { options: o, theme: t } = first.current;
      v = new GraphView(el, o, t === undefined ? {} : { theme: t });
      view.current = v;
    }
    v.bind();
    const made = v;
    ready.current?.({
      view: made,
      announce: (text) => {
        if (status.current !== null) status.current.textContent = text;
      },
    });
    return () => {
      ready.current?.(null);
      made.unbind();
    };
  }, [win]);

  useEffect(
    () => () => {
      view.current?.destroy();
      view.current = null;
    },
    [],
  );

  // The view was made with the first options; only later ones are news (a host that drives the
  // view itself has set its own by now, and must not get these back).
  const applied = useRef(options);
  useLayoutEffect(() => {
    if (applied.current === options) return;
    applied.current = options;
    view.current?.setOptions(options);
  }, [options]);
  useLayoutEffect(() => {
    if (view.current !== null && events !== undefined) view.current.events = events;
  }, [events]);
  useLayoutEffect(() => {
    if (curves !== undefined) view.current?.setCurves(curves);
  }, [curves]);
  useLayoutEffect(() => {
    if (selection !== undefined) view.current?.setSelection(selection);
  }, [selection]);
  useLayoutEffect(() => {
    if (playhead !== undefined) view.current?.setPlayhead(playhead);
  }, [playhead]);
  useLayoutEffect(() => {
    if (viewport !== undefined) view.current?.setViewport(viewport);
  }, [viewport]);
  useLayoutEffect(() => {
    if (normalized !== undefined) view.current?.setNormalized(normalized);
  }, [normalized]);
  useLayoutEffect(() => {
    if (tangentMode !== undefined) view.current?.setTangentMode(tangentMode);
  }, [tangentMode]);

  return (
    <div className={cx("cw-graph", className)}>
      {/* biome-ignore lint/a11y/noInteractiveElementToNoninteractiveRole: the canvas is the widget: it takes the focus and every key, which is what `application` says */}
      <canvas
        ref={canvas}
        className="cw-graph__canvas"
        role="application"
        tabIndex={0}
        aria-label={labels.graph}
        aria-describedby={helpId}
      />
      <span id={helpId} className="cw-visually-hidden">
        {labels.help}
      </span>
      <div ref={status} className="cw-visually-hidden" role="status" aria-live="polite" />
    </div>
  );
}
