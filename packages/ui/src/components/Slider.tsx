// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixSlider from "@radix-ui/react-slider";
import { useId } from "react";
import { cx } from "./cx.ts";
import { KeyframeGroup, type KeyframeState } from "./KeyframeToggle.tsx";

export interface SliderKeyframe {
  state: KeyframeState;
  onToggle: () => void;
  onPrev?: (() => void) | undefined;
  onNext?: (() => void) | undefined;
  /** Property cannot animate; the slot stays reserved but disabled. */
  disabled?: boolean | undefined;
}

export interface SliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  /** Live value while dragging or stepping. */
  onChange: (value: number) => void;
  /** Once per gesture (pointer release or key press). */
  onCommit?: ((value: number) => void) | undefined;
  /** Readout text; defaults to the plain number. */
  format?: ((value: number) => string) | undefined;
  disabled?: boolean | undefined;
  /** Built-in KeyframeToggle (with prev/next when handlers are given). */
  keyframe?: SliderKeyframe | undefined;
  className?: string | undefined;
}

/** Bounded value with a track and an always-visible readout. Shift+Arrow steps x10. */
export function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
  onCommit,
  format,
  disabled,
  keyframe,
  className,
}: SliderProps) {
  const labelId = useId();
  const text = format ? format(value) : String(value);
  return (
    <div className={cx("cw-slider", className)} aria-disabled={disabled ? "true" : undefined}>
      <span id={labelId} className="cw-slider__label label">
        {label}
      </span>
      <RadixSlider.Root
        className="cw-slider__track"
        value={[value]}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        onValueChange={([next]) => {
          if (next !== undefined) onChange(next);
        }}
        onValueCommit={([next]) => {
          if (next !== undefined) onCommit?.(next);
        }}
      >
        <RadixSlider.Track className="cw-slider__rail">
          <RadixSlider.Range className="cw-slider__fill" />
        </RadixSlider.Track>
        <RadixSlider.Thumb
          className="cw-slider__thumb"
          aria-labelledby={labelId}
          aria-valuetext={text}
        />
      </RadixSlider.Root>
      <span className="cw-slider__value numeric">{text}</span>
      {keyframe ? (
        <KeyframeGroup
          property={label}
          state={keyframe.state}
          onToggle={keyframe.onToggle}
          onPrev={keyframe.onPrev}
          onNext={keyframe.onNext}
          disabled={keyframe.disabled || disabled}
        />
      ) : null}
    </div>
  );
}
