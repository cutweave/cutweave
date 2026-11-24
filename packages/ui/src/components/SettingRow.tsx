// SPDX-License-Identifier: GPL-3.0-or-later
import type { ReactNode } from "react";
import { cx } from "./cx.ts";
import { IconButton } from "./IconButton.tsx";

export interface SettingRowProps {
  /** The row's id: the label and the description get `${id}-label` and `${id}-description`. */
  id: string;
  /** The setting's name: "Theme". Also names the reset button. */
  label: string;
  /** What the label shows instead of the plain text (a search match marked with `<mark>`). */
  labelContent?: ReactNode;
  /** One line on what it does: "Dark, light, or follow your system." */
  description?: string | undefined;
  /** What the description shows instead of the plain text. */
  descriptionContent?: ReactNode;
  /** Id of the control, so a click on the label reaches it. Omit when the control names itself. */
  htmlFor?: string | undefined;
  /** The value differs from its default: the reset button appears. */
  modified?: boolean | undefined;
  /** Puts this setting back to its default. */
  onReset?: (() => void) | undefined;
  /** Name of the reset button: "Reset Theme". Defaults to `Reset ${label}`. */
  resetLabel?: string | undefined;
  /** A short condition under the description: "Needs the engine", "Applies at the next start". */
  note?: string | undefined;
  /** The row cannot be changed now (its control is disabled by the host as well). */
  disabled?: boolean | undefined;
  /** The row a deep link or a search result points at. */
  highlighted?: boolean | undefined;
  /** The control: a switch, a segmented control, a select, a number field, a button. */
  children: ReactNode;
  className?: string | undefined;
}

/**
 * One setting in the Settings dialog: its name and one-line description on the left, its control
 * on the right, and a reset button only while the value differs from its default. A fixed slot
 * holds the reset button, so the controls line up whether or not it shows.
 */
export function SettingRow({
  id,
  label,
  labelContent,
  description,
  descriptionContent,
  htmlFor,
  modified,
  onReset,
  resetLabel,
  note,
  disabled,
  highlighted,
  children,
  className,
}: SettingRowProps) {
  const labelId = `${id}-label`;
  const descriptionId = `${id}-description`;
  const text = labelContent ?? label;
  return (
    <div
      className={cx("cw-setting", className)}
      data-setting={id}
      data-modified={modified ? "true" : undefined}
      data-disabled={disabled ? "true" : undefined}
      data-highlighted={highlighted ? "true" : undefined}
    >
      <div className="cw-setting__text">
        {htmlFor ? (
          <label id={labelId} htmlFor={htmlFor} className="cw-setting__label label">
            {text}
          </label>
        ) : (
          <span id={labelId} className="cw-setting__label label">
            {text}
          </span>
        )}
        {description ? (
          <p id={descriptionId} className="cw-setting__description caption">
            {descriptionContent ?? description}
          </p>
        ) : null}
        {note ? <p className="cw-setting__note caption">{note}</p> : null}
      </div>
      <div className="cw-setting__control">{children}</div>
      <div className="cw-setting__reset">
        {modified && onReset ? (
          <IconButton
            size="sm"
            icon="undo-2"
            label={resetLabel ?? `Reset ${label}`}
            onClick={onReset}
          />
        ) : null}
      </div>
    </div>
  );
}
