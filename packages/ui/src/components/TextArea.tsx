// SPDX-License-Identifier: GPL-3.0-or-later
import { forwardRef, type TextareaHTMLAttributes } from "react";
import { cx } from "./cx.ts";

export interface TextAreaProps
  extends Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, "value" | "onChange"> {
  value: string;
  /** Every keystroke. The host decides when a value is committed (blur, Ctrl+Enter). */
  onChange: (value: string) => void;
  /** Names the field when no `<label htmlFor>` does. */
  "aria-label"?: string | undefined;
  /** The text is not valid (too long): red edge, `aria-invalid`. */
  invalid?: boolean | undefined;
}

/**
 * Multi-line plain text on `surface-2` (a note), with the marigold edge while focused. It grows
 * only when the person drags its corner; Enter makes a new line.
 */
export const TextArea = forwardRef<HTMLTextAreaElement, TextAreaProps>(function TextArea(
  { value, onChange, invalid, className, rows = 3, ...rest },
  ref,
) {
  return (
    <textarea
      ref={ref}
      value={value}
      rows={rows}
      spellCheck={false}
      autoComplete="off"
      aria-invalid={invalid ? "true" : undefined}
      className={cx("cw-field cw-field--multiline body", className)}
      onChange={(event) => onChange(event.target.value)}
      {...rest}
    />
  );
});
