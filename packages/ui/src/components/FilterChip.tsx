// SPDX-License-Identifier: GPL-3.0-or-later
import { Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";

export interface FilterChipProps {
  /** What is filtered: "Audio", "Not used", "10 s to 1 min". */
  label: string;
  /** Name of the remove button: "Remove filter Audio". */
  removeLabel: string;
  onRemove: () => void;
  className?: string | undefined;
}

/** One active filter, with a button that removes it. */
export function FilterChip({ label, removeLabel, onRemove, className }: FilterChipProps) {
  return (
    <span className={cx("cw-chip", className)}>
      <span className="cw-chip__label label">{label}</span>
      <button type="button" className="cw-chip__remove" aria-label={removeLabel} onClick={onRemove}>
        <Icon name="x" size="sm" />
      </button>
    </span>
  );
}
