// SPDX-License-Identifier: GPL-3.0-or-later
import {
  type ButtonHTMLAttributes,
  forwardRef,
  type HTMLAttributes,
  type KeyboardEvent,
} from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";

export interface IconButtonProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "aria-label" | "children"> {
  icon: IconName;
  /** Accessible name; include the shortcut, e.g. "Razor (C)". */
  label: string;
  /** `lg` = control-lg with icon-lg (toolbar, default); `sm` = control-sm with icon-md. */
  size?: "sm" | "lg";
  /** Toggle state, mirrored to `aria-pressed`. Omit for a plain action button. */
  pressed?: boolean;
  /**
   * A small count on the corner (active filters). Shown from 1; it is decoration, so put the count
   * in `label` too ("Filters, 2 active").
   */
  badge?: number | undefined;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { icon, label, size = "lg", pressed, badge, className, type = "button", ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type}
      aria-label={label}
      aria-pressed={pressed}
      className={cx("cw-iconbtn", size === "sm" && "cw-iconbtn--sm", className)}
      {...rest}
    >
      <Icon name={icon} size={size === "sm" ? "md" : "lg"} />
      {badge !== undefined && badge > 0 ? (
        <span className="cw-iconbtn__badge caption" aria-hidden="true">
          {badge > 9 ? "9+" : badge}
        </span>
      ) : null}
    </button>
  );
});

/** Toolbar surface with arrow-key roving focus between its enabled buttons. */
export function Toolbar({ className, onKeyDown, ...rest }: HTMLAttributes<HTMLDivElement>) {
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    onKeyDown?.(event);
    if (event.defaultPrevented) return;
    const { key } = event;
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(key)) return;
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled)"),
    );
    if (items.length === 0) return;
    const index = items.indexOf(document.activeElement as HTMLElement);
    let next = index;
    if (key === "ArrowRight") next = (index + 1) % items.length;
    else if (key === "ArrowLeft") next = (index - 1 + items.length) % items.length;
    else if (key === "Home") next = 0;
    else next = items.length - 1;
    event.preventDefault();
    items[next]?.focus();
  };
  return (
    <div
      role="toolbar"
      className={cx("cw-toolbar", className)}
      onKeyDown={handleKeyDown}
      {...rest}
    />
  );
}

export function ToolbarSeparator() {
  return <hr className="cw-toolbar__sep" />;
}
