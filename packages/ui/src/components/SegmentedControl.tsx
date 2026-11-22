// SPDX-License-Identifier: GPL-3.0-or-later
import * as ToggleGroup from "@radix-ui/react-toggle-group";
import { cx } from "./cx.ts";

export interface SegmentedOption<T extends string = string> {
  value: T;
  label: string;
  disabled?: boolean | undefined;
}

export interface SegmentedControlProps<T extends string = string> {
  options: readonly SegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  /** Accessible name for the group. */
  "aria-label": string;
  disabled?: boolean | undefined;
  className?: string | undefined;
}

/**
 * Exactly-one-of-N picker. `role="radiogroup"` with `role="radio"` items;
 * Arrow keys move focus and selection follows focus.
 */
export function SegmentedControl<T extends string = string>({
  options,
  value,
  onChange,
  "aria-label": ariaLabel,
  disabled,
  className,
}: SegmentedControlProps<T>) {
  const select = (next: string) => {
    // A single-select group reports "" when the active item is pressed again; ignore it.
    if (next === "" || next === value) return;
    onChange(next as T);
  };
  return (
    <ToggleGroup.Root
      type="single"
      role="radiogroup"
      aria-label={ariaLabel}
      value={value}
      onValueChange={select}
      disabled={disabled}
      className={cx("cw-seg", className)}
    >
      {options.map((option) => (
        <ToggleGroup.Item
          key={option.value}
          value={option.value}
          disabled={option.disabled}
          className="cw-seg__item label"
          onFocus={() => select(option.value)}
        >
          {option.label}
        </ToggleGroup.Item>
      ))}
    </ToggleGroup.Root>
  );
}
