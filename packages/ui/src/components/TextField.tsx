// SPDX-License-Identifier: GPL-3.0-or-later
import { forwardRef, type InputHTMLAttributes } from "react";
import { cx } from "./cx.ts";

export interface TextFieldProps
  extends Omit<InputHTMLAttributes<HTMLInputElement>, "value" | "onChange" | "size"> {
  value: string;
  /** Every keystroke. The host decides when a value is committed (Enter or blur). */
  onChange: (value: string) => void;
  /** Names the field when no `<label htmlFor>` does. */
  "aria-label"?: string | undefined;
  /** The text is not valid (a timecode that does not parse): red edge, `aria-invalid`. */
  invalid?: boolean | undefined;
  /** Timecodes and other numbers: mono, tabular digits, right-aligned. */
  numeric?: boolean | undefined;
}

/**
 * Single-line text input on `surface-2`, `control-sm` tall, with the marigold edge while
 * focused. A mixed selection shows an empty field with a placeholder ("Mixed").
 */
export const TextField = forwardRef<HTMLInputElement, TextFieldProps>(function TextField(
  { value, onChange, invalid, numeric, className, type = "text", ...rest },
  ref,
) {
  return (
    <input
      ref={ref}
      type={type}
      value={value}
      spellCheck={false}
      autoComplete="off"
      aria-invalid={invalid ? "true" : undefined}
      className={cx("cw-field", numeric ? "numeric cw-field--numeric" : "body", className)}
      onChange={(event) => onChange(event.target.value)}
      {...rest}
    />
  );
});
