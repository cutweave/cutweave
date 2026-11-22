// SPDX-License-Identifier: GPL-3.0-or-later
import * as Collapsible from "@radix-ui/react-collapsible";
import type { ReactNode } from "react";
import { Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";
import { IconButton } from "./IconButton.tsx";

export interface InspectorSectionProps {
  /** Section name; rendered in the overline style. */
  label: string;
  expanded: boolean;
  /** UI-only; persisted per workspace via `layout.section.set`. */
  onExpandedChange: (expanded: boolean) => void;
  /** Any value in the section differs from its default; shows the section reset. */
  modified?: boolean | undefined;
  /** Section reset. Dispatches `clip.resetProperties {group}`. */
  onReset?: (() => void) | undefined;
  children: ReactNode;
  className?: string | undefined;
}

/** Collapsible group of property rows with a section reset. Do not nest. */
export function InspectorSection({
  label,
  expanded,
  onExpandedChange,
  modified,
  onReset,
  children,
  className,
}: InspectorSectionProps) {
  return (
    <Collapsible.Root
      open={expanded}
      onOpenChange={onExpandedChange}
      className={cx("cw-section", className)}
    >
      <div className="cw-section__head">
        <Collapsible.Trigger className="cw-section__toggle overline">
          <Icon name="chevron-down" size="md" className="cw-section__chevron" />
          {label}
        </Collapsible.Trigger>
        {modified && onReset ? (
          <IconButton size="sm" icon="undo-2" label={`Reset ${label}`} onClick={onReset} />
        ) : null}
      </div>
      <Collapsible.Content className="cw-section__body">{children}</Collapsible.Content>
    </Collapsible.Root>
  );
}

export interface InspectorRowProps {
  label: string;
  /** Id of the control, so the label names it. Omit when the control labels itself. */
  htmlFor?: string | undefined;
  /** The value differs from its default: label moves to `text-primary` and reset appears. */
  modified?: boolean | undefined;
  /** Row reset. Dispatches `clip.resetProperty {prop}`. */
  onReset?: (() => void) | undefined;
  children: ReactNode;
}

/** Label left, control right, reset only while modified (so it is only then in the tab order). */
export function InspectorRow({ label, htmlFor, modified, onReset, children }: InspectorRowProps) {
  const labelClass = "cw-row__label label";
  return (
    <div className="cw-row" data-modified={modified ? "true" : undefined}>
      {htmlFor ? (
        <label htmlFor={htmlFor} className={labelClass}>
          {label}
        </label>
      ) : (
        <span className={labelClass}>{label}</span>
      )}
      <span className="cw-row__control">
        {children}
        {modified && onReset ? (
          <IconButton
            size="sm"
            icon="undo-2"
            label={`Reset ${label}`}
            className="cw-row__reset"
            onClick={onReset}
          />
        ) : null}
      </span>
    </div>
  );
}
