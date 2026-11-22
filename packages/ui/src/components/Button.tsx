// SPDX-License-Identifier: GPL-3.0-or-later
import { type ButtonHTMLAttributes, forwardRef } from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";

export type ButtonVariant = "secondary" | "primary" | "ghost" | "danger" | "agent";
export type ButtonSize = "sm" | "md" | "lg";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** `primary` once per surface; `agent` only for accepting agent work. */
  variant?: ButtonVariant;
  /** `sm` = control-sm, `md` = control-md (default), `lg` = control-lg (dialogs, AI Studio). */
  size?: ButtonSize;
  /** Optional leading icon. Icon-only actions must use IconButton. */
  icon?: IconName;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "secondary", size = "md", icon, className, type = "button", children, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type}
      className={cx(
        "cw-btn",
        size === "sm" ? "label" : "body-strong",
        variant !== "secondary" && `cw-btn--${variant}`,
        size !== "md" && `cw-btn--${size}`,
        className,
      )}
      {...rest}
    >
      {icon ? <Icon name={icon} size="md" /> : null}
      {children}
    </button>
  );
});
