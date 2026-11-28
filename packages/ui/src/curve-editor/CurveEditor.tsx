// SPDX-License-Identifier: GPL-3.0-or-later
// The curve editor (WP-2C.05): how one keyframe's segment moves to the next key. A kind switch
// (hold, linear, bezier, spring), a preset picker, a graph of progress over time with the two
// bezier handles, and the spring's numbers. The curve drawn is sampled by the caller (`sample`),
// which the editor wires to the engine's own math in the shared wasm module, so the preview is
// what the engine renders. Everything works from the keyboard: the handles are focusable and the
// arrow keys nudge them (Shift: ten times as far).
import {
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { cx } from "../components/cx.ts";
import { NumberScrubber } from "../components/NumberScrubber.tsx";
import { SegmentedControl, type SegmentedOption } from "../components/SegmentedControl.tsx";
import { Switch } from "../components/Switch.tsx";
import {
  clampHandle,
  curvePath,
  HANDLE_STEP,
  PLOT,
  pointFromClient,
  toX,
  toY,
  Y_MAX,
  Y_MIN,
} from "./geometry.ts";

export type CurveKind = "hold" | "linear" | "bezier" | "spring";

/** A segment's interpolation, in the engine's stored form (`Interp`). */
export type CurveValue =
  | { readonly kind: "hold" }
  | { readonly kind: "linear" }
  | {
      readonly kind: "bezier";
      readonly x1: number;
      readonly y1: number;
      readonly x2: number;
      readonly y2: number;
      readonly preset?: string | null;
    }
  | {
      readonly kind: "spring";
      readonly stiffness: number;
      readonly damping: number;
      readonly mass: number;
      readonly bounce: boolean;
      readonly preset?: string | null;
    };

export interface CurvePreset {
  /** The engine's easing name (`ease_in_out`, `spring_gentle`, ...). */
  readonly id: string;
  readonly label: string;
  readonly value: CurveValue;
}

/** Every visible string; override per locale. */
export interface CurveEditorLabels {
  kind: string;
  hold: string;
  linear: string;
  bezier: string;
  spring: string;
  preset: string;
  custom: string;
  graph: string;
  time: string;
  value: string;
  handleStart: string;
  handleEnd: string;
  /** The spoken position of a handle. */
  handleAt: (time: string, value: string) => string;
  stiffness: string;
  damping: string;
  mass: string;
  bounce: string;
}

export const DEFAULT_CURVE_LABELS: CurveEditorLabels = {
  kind: "Interpolation",
  hold: "Hold",
  linear: "Linear",
  bezier: "Bezier",
  spring: "Spring",
  preset: "Preset",
  custom: "Custom",
  graph: "Easing curve",
  time: "Time",
  value: "Value",
  handleStart: "Start handle",
  handleEnd: "End handle",
  handleAt: (time, value) => `time ${time}, value ${value}`,
  stiffness: "Stiffness",
  damping: "Damping",
  mass: "Mass",
  bounce: "Bounce",
};

export interface CurveEditorProps {
  value: CurveValue;
  /** Live value while a handle or a number is dragged (the preview follows it by itself). */
  onChange?: ((value: CurveValue) => void) | undefined;
  /** Once per gesture: a handle released, an arrow key, a preset or a kind picked. */
  onCommit: (value: CurveValue) => void;
  presets: readonly CurvePreset[];
  /**
   * Progress `p(u)` at `n` evenly spaced times from the key (`u = 0`) to the next key (`u = 1`),
   * from the engine's math (the shared wasm module).
   */
  sample: (value: CurveValue, n: number) => ArrayLike<number>;
  /** The kinds the property accepts (a flip takes only `hold`). Default: all four. */
  kinds?: readonly CurveKind[] | undefined;
  disabled?: boolean | undefined;
  labels?: Partial<CurveEditorLabels> | undefined;
  className?: string | undefined;
}

const ALL_KINDS: readonly CurveKind[] = ["hold", "linear", "bezier", "spring"];
const KIND_ICON = {
  hold: "ease-hold",
  linear: "ease-linear",
  bezier: "ease-bezier",
  spring: "ease-spring",
} as const;
/** Samples of the preview curve. */
const SAMPLES = 64;
const DEFAULT_BEZIER: CurveValue = { kind: "bezier", x1: 0.42, y1: 0, x2: 0.58, y2: 1 };
const DEFAULT_SPRING: CurveValue = {
  kind: "spring",
  stiffness: 170,
  damping: 26,
  mass: 1,
  bounce: false,
};

const fmt = (n: number): string => n.toFixed(2);

/** The value a kind switch lands on: the first preset of that kind, else a plain default. */
function valueOfKind(kind: CurveKind, presets: readonly CurvePreset[]): CurveValue {
  if (kind === "hold" || kind === "linear") return { kind };
  const wanted = kind === "bezier" ? "ease_in_out" : "spring";
  const preset =
    presets.find((p) => p.id === wanted && p.value.kind === kind) ??
    presets.find((p) => p.value.kind === kind);
  return preset?.value ?? (kind === "bezier" ? DEFAULT_BEZIER : DEFAULT_SPRING);
}

/** The preset whose numbers are exactly the value's, or null ("Custom"). */
function presetOf(value: CurveValue, presets: readonly CurvePreset[]): CurvePreset | null {
  const same = (a: CurveValue, b: CurveValue): boolean => {
    if (a.kind !== b.kind) return false;
    if (a.kind === "bezier" && b.kind === "bezier") {
      return a.x1 === b.x1 && a.y1 === b.y1 && a.x2 === b.x2 && a.y2 === b.y2;
    }
    if (a.kind === "spring" && b.kind === "spring") {
      return (
        a.stiffness === b.stiffness &&
        a.damping === b.damping &&
        a.mass === b.mass &&
        a.bounce === b.bounce
      );
    }
    return true;
  };
  return presets.find((p) => same(p.value, value)) ?? null;
}

type Handle = 1 | 2;

export function CurveEditor({
  value,
  onChange,
  onCommit,
  presets,
  sample,
  kinds = ALL_KINDS,
  disabled = false,
  labels: given,
  className,
}: CurveEditorProps) {
  const labels = useMemo(() => ({ ...DEFAULT_CURVE_LABELS, ...given }), [given]);
  const presetId = useId();
  const svg = useRef<SVGSVGElement>(null);
  // The value under a drag that has not been committed yet.
  const [draft, setDraft] = useState<CurveValue | null>(null);
  const drag = useRef<{ handle: Handle; pointer: number } | null>(null);
  const shown = draft ?? value;

  const live = (next: CurveValue): void => {
    setDraft(next);
    onChange?.(next);
  };
  const commit = (next: CurveValue): void => {
    setDraft(null);
    onCommit(next);
  };

  const path = useMemo(() => curvePath(sample(shown, SAMPLES)), [sample, shown]);
  const current = presetOf(shown, presets);
  const kindOptions = useMemo<SegmentedOption<CurveKind>[]>(
    () =>
      ALL_KINDS.map((kind) => ({
        value: kind,
        label: labels[kind],
        icon: KIND_ICON[kind],
        disabled: !kinds.includes(kind),
      })),
    [kinds, labels],
  );

  const withHandle = (handle: Handle, x: number, y: number): CurveValue => {
    if (shown.kind !== "bezier") return shown;
    const p = clampHandle(x, y);
    const { preset: _preset, ...numbers } = shown;
    return handle === 1 ? { ...numbers, x1: p.x, y1: p.y } : { ...numbers, x2: p.x, y2: p.y };
  };

  const onHandleDown = (handle: Handle) => (e: ReactPointerEvent<SVGGElement>) => {
    if (disabled || e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.focus();
    e.currentTarget.setPointerCapture?.(e.pointerId);
    drag.current = { handle, pointer: e.pointerId };
  };
  const onHandleMove = (e: ReactPointerEvent<SVGGElement>): void => {
    const d = drag.current;
    const el = svg.current;
    if (d === null || el === null || d.pointer !== e.pointerId) return;
    const p = pointFromClient(el.getBoundingClientRect(), e.clientX, e.clientY);
    live(withHandle(d.handle, p.x, p.y));
  };
  const onHandleUp = (e: ReactPointerEvent<SVGGElement>): void => {
    const d = drag.current;
    if (d === null || d.pointer !== e.pointerId) return;
    drag.current = null;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
    if (draft !== null) commit(draft);
  };
  const onHandleKey = (handle: Handle) => (e: KeyboardEvent<SVGGElement>) => {
    if (disabled || shown.kind !== "bezier") return;
    if (e.key === "Escape" && drag.current !== null) {
      // Abandon the drag: the committed value shows again.
      drag.current = null;
      setDraft(null);
      onChange?.(value);
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    const step = e.shiftKey ? HANDLE_STEP * 10 : HANDLE_STEP;
    const x = handle === 1 ? shown.x1 : shown.x2;
    const y = handle === 1 ? shown.y1 : shown.y2;
    let next: { x: number; y: number } | null = null;
    if (e.key === "ArrowLeft") next = { x: x - step, y };
    else if (e.key === "ArrowRight") next = { x: x + step, y };
    else if (e.key === "ArrowUp") next = { x, y: y + step };
    else if (e.key === "ArrowDown") next = { x, y: y - step };
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    // Round so repeated nudges do not accumulate float noise.
    commit(withHandle(handle, Math.round(next.x * 1000) / 1000, Math.round(next.y * 1000) / 1000));
  };

  const handle = (which: Handle, x: number, y: number, fromX: number, fromY: number) => (
    <>
      <line className="cw-curve__arm" x1={toX(fromX)} y1={toY(fromY)} x2={toX(x)} y2={toY(y)} />
      <g
        role="slider"
        tabIndex={disabled ? -1 : 0}
        className="cw-curve__handle"
        data-handle={which}
        aria-label={which === 1 ? labels.handleStart : labels.handleEnd}
        aria-valuemin={0}
        aria-valuemax={1}
        aria-valuenow={x}
        aria-valuetext={labels.handleAt(fmt(x), fmt(y))}
        aria-disabled={disabled || undefined}
        onPointerDown={onHandleDown(which)}
        onPointerMove={onHandleMove}
        onPointerUp={onHandleUp}
        onPointerCancel={onHandleUp}
        onKeyDown={onHandleKey(which)}
      >
        <circle className="cw-curve__hit" cx={toX(x)} cy={toY(y)} r={12} />
        <circle className="cw-curve__knob" cx={toX(x)} cy={toY(y)} r={5} />
      </g>
    </>
  );

  const springField = (
    key: "stiffness" | "damping" | "mass",
    min: number,
    max: number,
    step: number,
  ) => {
    if (shown.kind !== "spring") return null;
    const set = (n: number): CurveValue => {
      const { preset: _preset, ...numbers } = shown;
      return { ...numbers, [key]: n };
    };
    return (
      <NumberScrubber
        label={labels[key]}
        value={shown[key]}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        onChange={(n) => live(set(n))}
        onCommit={(n) => commit(set(n))}
        onCancel={() => setDraft(null)}
      />
    );
  };

  return (
    <div className={cx("cw-curve", className)} data-kind={shown.kind}>
      <div className="cw-curve__bar">
        <SegmentedControl
          aria-label={labels.kind}
          options={kindOptions}
          value={shown.kind}
          iconOnly
          disabled={disabled}
          onChange={(kind) => commit(valueOfKind(kind, presets))}
        />
        <label className="cw-curve__preset caption" htmlFor={presetId}>
          {labels.preset}
        </label>
        <select
          id={presetId}
          className="cw-curve__select body"
          value={current?.id ?? ""}
          disabled={disabled}
          onChange={(e) => {
            const preset = presets.find((p) => p.id === e.target.value);
            if (preset !== undefined) commit(preset.value);
          }}
        >
          {current === null ? (
            <option value="" disabled>
              {labels.custom}
            </option>
          ) : null}
          {presets
            .filter((p) => kinds.includes(p.value.kind))
            .map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
        </select>
      </div>
      <div className="cw-curve__plot">
        <span className="cw-curve__axis cw-curve__axis--value caption">{labels.value}</span>
        {/* biome-ignore lint/a11y/useSemanticElements: a graph, not a fieldset of form controls */}
        <svg
          ref={svg}
          className="cw-curve__graph"
          viewBox={`0 0 ${PLOT.width} ${PLOT.height}`}
          role="group"
          aria-label={labels.graph}
        >
          <rect
            className="cw-curve__unit"
            x={toX(0)}
            y={toY(1)}
            width={toX(1) - toX(0)}
            height={toY(0) - toY(1)}
          />
          {[0.25, 0.5, 0.75].map((u) => (
            <line
              key={`v${u}`}
              className="cw-curve__grid"
              x1={toX(u)}
              y1={toY(Y_MIN)}
              x2={toX(u)}
              y2={toY(Y_MAX)}
            />
          ))}
          {[0, 0.5, 1].map((p) => (
            <line
              key={`h${p}`}
              className="cw-curve__grid"
              x1={toX(0)}
              y1={toY(p)}
              x2={toX(1)}
              y2={toY(p)}
            />
          ))}
          <path className="cw-curve__line" d={path} data-testid="curve-preview" />
          {shown.kind === "bezier" ? (
            <>
              {handle(1, shown.x1, shown.y1, 0, 0)}
              {handle(2, shown.x2, shown.y2, 1, 1)}
            </>
          ) : null}
        </svg>
        <span className="cw-curve__axis cw-curve__axis--time caption">{labels.time}</span>
      </div>
      {shown.kind === "spring" ? (
        <div className="cw-curve__spring">
          {springField("stiffness", 1, 10000, 1)}
          {springField("damping", 0, 1000, 1)}
          {springField("mass", 0.1, 100, 0.1)}
          <span className="cw-curve__bounce label">
            {labels.bounce}
            <Switch
              aria-label={labels.bounce}
              checked={shown.bounce}
              disabled={disabled}
              onCheckedChange={(bounce) => {
                const { preset: _preset, ...numbers } = shown;
                commit({ ...numbers, bounce });
              }}
            />
          </span>
        </div>
      ) : null}
    </div>
  );
}
