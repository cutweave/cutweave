// SPDX-License-Identifier: GPL-3.0-or-later
import {
  type KeyboardEvent,
  type PointerEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { cx } from "./cx.ts";
import { clamp, formatNumber, modifierScale, parseEntry, stepValue } from "./numberMath.ts";

export interface NumberScrubberProps {
  /** Short visible label; doubles as the accessible name unless `aria-label` is given. */
  label?: string;
  "aria-label"?: string | undefined;
  value: number;
  /** The selection disagrees: shows an em dash and announces "mixed". Never cleared by focus. */
  mixed?: boolean;
  unit?: string | undefined;
  min?: number | undefined;
  max?: number | undefined;
  /** Base step for keys and drag. Integer steps stay exact integers. Default 1. */
  step?: number | undefined;
  /** Pixels of drag per step. Default 4. */
  sensitivity?: number | undefined;
  /** Fixed decimals for display. Default: at most 2, trailing zeros trimmed (the value itself stays exact). */
  precision?: number | undefined;
  /** Live value during a drag, key press or typed commit. */
  onChange: (value: number) => void;
  /** Once per gesture: drag release, key press or typed Enter/blur. Not called on cancel. */
  onCommit?: ((value: number) => void) | undefined;
  /** A drag was abandoned (Escape); `onChange` has already restored the start value. */
  onCancel?: (() => void) | undefined;
  disabled?: boolean | undefined;
  className?: string | undefined;
}

/** Movement (px) before a press becomes a scrub rather than a click. */
const DRAG_THRESHOLD = 3;

interface Gesture {
  startValue: number;
  current: number;
  startX: number;
  lastX: number;
  acc: number;
  moved: boolean;
  locked: boolean;
}

export function NumberScrubber({
  label,
  "aria-label": ariaLabel,
  value,
  mixed = false,
  unit,
  min = Number.NEGATIVE_INFINITY,
  max = Number.POSITIVE_INFINITY,
  step = 1,
  sensitivity = 4,
  precision,
  onChange,
  onCommit,
  onCancel,
  disabled = false,
  className,
}: NumberScrubberProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const gesture = useRef<Gesture | null>(null);
  const editDone = useRef(true);
  const initialDraft = useRef("");
  const restoreFocus = useRef(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");

  const name = ariaLabel ?? label;
  const shown = formatNumber(value, precision);

  const exitLock = useCallback(() => {
    if (document.pointerLockElement === rootRef.current) document.exitPointerLock?.();
  }, []);

  const cancelDrag = useCallback(() => {
    const g = gesture.current;
    if (!g) return;
    gesture.current = null;
    exitLock();
    if (g.moved && g.current !== g.startValue) onChange(g.startValue);
    if (g.moved) onCancel?.();
  }, [exitLock, onChange, onCancel]);

  // Escape (or any other way of losing the pointer lock) mid-drag cancels the gesture.
  useEffect(() => {
    const onLockChange = () => {
      const g = gesture.current;
      if (!g?.moved) return;
      if (document.pointerLockElement === rootRef.current) g.locked = true;
      else if (g.locked) cancelDrag();
    };
    document.addEventListener("pointerlockchange", onLockChange);
    return () => document.removeEventListener("pointerlockchange", onLockChange);
  }, [cancelDrag]);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    } else if (restoreFocus.current) {
      restoreFocus.current = false;
      rootRef.current?.focus();
    }
  }, [editing]);

  const startEdit = () => {
    if (disabled || editing) return;
    editDone.current = false;
    initialDraft.current = mixed ? "" : shown;
    setDraft(initialDraft.current);
    setEditing(true);
  };

  const finishEdit = (commit: boolean) => {
    if (editDone.current) return;
    editDone.current = true;
    restoreFocus.current = true;
    setEditing(false);
    // An untouched draft is the rounded display: keep the exact value rather than commit it.
    if (!commit || (!mixed && draft === initialDraft.current)) return;
    const n = parseEntry(draft, value, unit);
    if (Number.isNaN(n)) return;
    const next = clamp(n, min, max);
    if (!mixed && next === value) return;
    onChange(next);
    onCommit?.(next);
  };

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (disabled || editing || event.button !== 0) return;
    gesture.current = {
      startValue: value,
      current: value,
      startX: event.clientX,
      lastX: event.clientX,
      acc: 0,
      moved: false,
      locked: false,
    };
    event.currentTarget.setPointerCapture?.(event.pointerId);
    event.currentTarget.focus();
  };

  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const g = gesture.current;
    if (!g) return;
    if (!g.moved) {
      if (Math.abs(event.clientX - g.startX) < DRAG_THRESHOLD) return;
      g.moved = true;
      g.lastX = event.clientX;
      try {
        // Pointer lock keeps the scrub going past the screen edge; failure falls back to clientX.
        void Promise.resolve(rootRef.current?.requestPointerLock?.()).catch(() => {});
      } catch {
        // Not available (tests, some embedders).
      }
      return;
    }
    const locked = document.pointerLockElement === rootRef.current;
    const dx = locked ? event.movementX : event.clientX - g.lastX;
    g.lastX = event.clientX;
    g.acc += dx / sensitivity;
    const whole = Math.trunc(g.acc);
    if (whole === 0) return;
    g.acc -= whole;
    const next = stepValue(
      g.current,
      whole,
      step,
      modifierScale({ shift: event.shiftKey, alt: event.altKey }),
      min,
      max,
    );
    if (next === g.current) return;
    g.current = next;
    onChange(next);
  };

  const onPointerUp = (event: PointerEvent<HTMLDivElement>) => {
    const g = gesture.current;
    if (!g) return;
    gesture.current = null;
    exitLock();
    event.currentTarget.releasePointerCapture?.(event.pointerId);
    if (g.moved) onCommit?.(g.current);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (disabled || editing || event.target !== event.currentTarget) return;
    if (event.key === "Escape") {
      if (gesture.current?.moved) {
        event.preventDefault();
        cancelDrag();
      }
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      startEdit();
      return;
    }
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    const next = stepValue(
      value,
      event.key === "ArrowUp" ? 1 : -1,
      step,
      modifierScale({ shift: event.shiftKey, alt: event.altKey }),
      min,
      max,
    );
    if (next === value && !mixed) return;
    onChange(next);
    onCommit?.(next);
  };

  const spin = editing
    ? {}
    : ({
        role: "spinbutton",
        tabIndex: disabled ? -1 : 0,
        "aria-label": name,
        "aria-valuenow": mixed ? undefined : value,
        "aria-valuetext": mixed ? "mixed" : unit ? `${shown} ${unit}` : undefined,
        "aria-valuemin": Number.isFinite(min) ? min : undefined,
        "aria-valuemax": Number.isFinite(max) ? max : undefined,
        "aria-disabled": disabled ? "true" : undefined,
      } as const);

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: role=spinbutton is applied via the spread (absent while the inline input is editing)
    <div
      ref={rootRef}
      className={cx("cw-num", className)}
      data-state={editing ? "editing" : mixed ? "mixed" : undefined}
      {...spin}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={cancelDrag}
      onDoubleClick={startEdit}
      onKeyDown={onKeyDown}
    >
      {label ? <span className="cw-num__label label">{label}</span> : null}
      {editing ? (
        <input
          ref={inputRef}
          className="cw-num__value numeric"
          aria-label={name}
          inputMode="decimal"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              finishEdit(true);
            } else if (event.key === "Escape") {
              event.preventDefault();
              finishEdit(false);
            }
          }}
          onBlur={() => finishEdit(true)}
        />
      ) : (
        <span className="cw-num__value numeric">{mixed ? "—" : shown}</span>
      )}
      {unit ? <span className="cw-num__unit numeric">{unit}</span> : null}
    </div>
  );
}
