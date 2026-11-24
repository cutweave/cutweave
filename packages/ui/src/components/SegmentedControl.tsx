// SPDX-License-Identifier: GPL-3.0-or-later
import * as ToggleGroup from "@radix-ui/react-toggle-group";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";
import { Tooltip } from "./Tooltip.tsx";

export interface SegmentedOption<T extends string = string> {
  value: T;
  /** The visible text, or with `iconOnly` the accessible name and tooltip. */
  label: string;
  /** Shown before the label, or alone with `iconOnly`. */
  icon?: IconName | undefined;
  disabled?: boolean | undefined;
}

export interface SegmentedControlProps<T extends string = string> {
  options: readonly SegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  /** Accessible name for the group. */
  "aria-label": string;
  disabled?: boolean | undefined;
  /**
   * Icons only, for tight bars: each option shows its `icon`, and its `label` becomes the
   * accessible name and a tooltip.
   */
  iconOnly?: boolean | undefined;
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
  iconOnly = false,
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
      className={cx("cw-seg", iconOnly && "cw-seg--icons", className)}
    >
      {options.map((option) => {
        const iconic = iconOnly && option.icon !== undefined;
        const item = (
          <ToggleGroup.Item
            key={option.value}
            value={option.value}
            disabled={option.disabled}
            aria-label={iconic ? option.label : undefined}
            className={cx("cw-seg__item label", iconic && "cw-seg__item--icon")}
            onFocus={() => select(option.value)}
          >
            {option.icon === undefined ? null : <Icon name={option.icon} size="md" />}
            {iconic ? null : option.label}
          </ToggleGroup.Item>
        );
        return iconic ? (
          <Tooltip key={option.value} label={option.label}>
            {item}
          </Tooltip>
        ) : (
          item
        );
      })}
    </ToggleGroup.Root>
  );
}
