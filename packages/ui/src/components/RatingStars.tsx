// SPDX-License-Identifier: GPL-3.0-or-later
import { type KeyboardEvent, useRef } from "react";
import { Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";
import { Tooltip } from "./Tooltip.tsx";

export interface RatingStarsProps {
  /** Whole stars, 1 to `max`; `null`: unrated (or a selection whose ratings differ). */
  value: number | null;
  /** A star was chosen; `null` when the current one was chosen again (the rating is cleared). */
  onChange: (value: number | null) => void;
  /** Accessible name for the group: "Rating". */
  "aria-label": string;
  /** Names one star: `(3) => "3 stars"`. Also its tooltip. */
  starLabel: (stars: number) => string;
  max?: number | undefined;
  disabled?: boolean | undefined;
  className?: string | undefined;
}

/**
 * A rating in whole stars. `role="radiogroup"` with one `role="radio"` per star: click a star to
 * set it, click the current one again to clear. Arrow keys move and set (Left from one star
 * clears), Home and End go to the ends, Delete or Backspace clears. A set star is an "on" state,
 * so it is drawn like a pressed toggle (`marigold-text`).
 */
export function RatingStars({
  value,
  onChange,
  "aria-label": ariaLabel,
  starLabel,
  max = 5,
  disabled = false,
  className,
}: RatingStarsProps) {
  const root = useRef<HTMLDivElement>(null);
  const stars = Array.from({ length: max }, (_, i) => i + 1);
  // The one star in the tab order: the current one, else the first.
  const tabStop = value ?? 1;

  const set = (next: number | null) => {
    if (disabled || next === value) return;
    onChange(next);
  };
  const focusStar = (n: number) =>
    root.current?.querySelector<HTMLElement>(`[data-star="${n}"]`)?.focus();

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (disabled || event.altKey || event.ctrlKey || event.metaKey) return;
    const at = value ?? 0;
    let next: number | null | undefined;
    switch (event.key) {
      case "ArrowRight":
      case "ArrowUp":
        next = Math.min(max, at + 1);
        break;
      case "ArrowLeft":
      case "ArrowDown":
        next = at <= 1 ? null : at - 1;
        break;
      case "Home":
        next = 1;
        break;
      case "End":
        next = max;
        break;
      case "Delete":
      case "Backspace":
        next = null;
        break;
      default:
        return;
    }
    event.preventDefault();
    set(next);
    focusStar(next ?? 1);
  };

  return (
    <div
      ref={root}
      role="radiogroup"
      aria-label={ariaLabel}
      aria-disabled={disabled ? "true" : undefined}
      className={cx("cw-stars", className)}
      onKeyDown={onKeyDown}
    >
      {stars.map((n) => {
        const label = starLabel(n);
        return (
          <Tooltip key={n} label={label}>
            {/* biome-ignore lint/a11y/useSemanticElements: see the group */}
            <button
              type="button"
              role="radio"
              aria-checked={value === n}
              aria-label={label}
              tabIndex={n === tabStop ? 0 : -1}
              disabled={disabled}
              data-star={n}
              data-filled={value !== null && n <= value ? "true" : undefined}
              className="cw-stars__star"
              onClick={() => set(value === n ? null : n)}
            >
              <Icon name="star" size="md" className="cw-stars__icon" />
            </button>
          </Tooltip>
        );
      })}
    </div>
  );
}
