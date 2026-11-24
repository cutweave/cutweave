// SPDX-License-Identifier: GPL-3.0-or-later
import {
  forwardRef,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type Ref,
  useImperativeHandle,
  useRef,
} from "react";
import { Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";
import { IconButton } from "./IconButton.tsx";
import { Tooltip } from "./Tooltip.tsx";

export interface SearchFieldProps
  extends Omit<
    InputHTMLAttributes<HTMLInputElement>,
    "value" | "onChange" | "size" | "type" | "aria-label"
  > {
  value: string;
  /** Every keystroke (the field filters as you type; there is no submit). */
  onChange: (value: string) => void;
  /** Accessible name of the field: "Search media". It is also the default placeholder. */
  label: string;
  /** Name of the clear button: "Clear search". */
  clearLabel: string;
  /** Shown instead of `label` while the field is empty. */
  placeholder?: string | undefined;
  /**
   * A short result summary at the right edge ("12 of 340"). It is announced politely when it
   * changes. Leave it out when the host shows the count elsewhere.
   */
  count?: string | undefined;
  /**
   * Esc on an empty field: leave it (the host moves focus on). Default: blur. Esc on a field with
   * text clears the text and stays.
   */
  onExit?: (() => void) | undefined;
  /** Arrow Down: move into the results. */
  onArrowDown?: (() => void) | undefined;
}

/**
 * A single-line search box: a magnifier, a clear button once there is text, an optional result
 * count. Esc clears, a second Esc leaves; Arrow Down hands focus to the results.
 */
export const SearchField = forwardRef<HTMLInputElement, SearchFieldProps>(function SearchField(
  {
    value,
    onChange,
    label,
    clearLabel,
    placeholder,
    count,
    onExit,
    onArrowDown,
    className,
    onKeyDown,
    ...rest
  },
  ref: Ref<HTMLInputElement>,
) {
  const input = useRef<HTMLInputElement>(null);
  useImperativeHandle(ref, () => input.current as HTMLInputElement);
  const active = value !== "";
  const counted = active && count !== undefined && count !== "";

  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    onKeyDown?.(event);
    if (event.defaultPrevented) return;
    if (event.key === "Escape") {
      event.preventDefault();
      // The first Esc clears; keep it from also closing a dialog or panel around the field.
      event.stopPropagation();
      if (active) onChange("");
      else if (onExit) onExit();
      else input.current?.blur();
    } else if (event.key === "ArrowDown" && onArrowDown) {
      event.preventDefault();
      onArrowDown();
    }
  };

  return (
    <div
      className={cx(
        "cw-search",
        active && "cw-search--active",
        counted && "cw-search--counted",
        className,
      )}
    >
      <Icon name="search" size="md" className="cw-search__icon" />
      <input
        ref={input}
        type="search"
        value={value}
        spellCheck={false}
        autoComplete="off"
        aria-label={label}
        placeholder={placeholder ?? label}
        className="cw-field cw-search__input body"
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={handleKeyDown}
        {...rest}
      />
      {counted ? (
        <span className="cw-search__count caption" role="status">
          {count}
        </span>
      ) : null}
      {active ? (
        <Tooltip label={clearLabel}>
          <IconButton
            size="sm"
            icon="x"
            label={clearLabel}
            className="cw-search__clear"
            onClick={() => {
              onChange("");
              input.current?.focus();
            }}
          />
        </Tooltip>
      ) : null}
    </div>
  );
});
