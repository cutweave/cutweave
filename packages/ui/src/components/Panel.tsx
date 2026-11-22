// SPDX-License-Identifier: GPL-3.0-or-later
import { type ReactNode, useId } from "react";
import { cx } from "./cx.ts";

export interface PanelProps {
  title: string;
  /** A `TabList` (inside a `Tabs`), shown in the header after the title. */
  tabs?: ReactNode;
  /** Up to three `IconButton size="sm"`, pushed to the right edge of the header. */
  actions?: ReactNode;
  /** Header only; the body is removed from the tree. Dispatches `layout.panel.collapse`. */
  collapsed?: boolean | undefined;
  children?: ReactNode;
  className?: string | undefined;
}

/** Docked region: fixed-height header (title, tabs, actions) above a `surface-1` body. */
export function Panel({ title, tabs, actions, collapsed, children, className }: PanelProps) {
  const titleId = useId();
  return (
    <section
      aria-labelledby={titleId}
      className={cx("cw-panel", className)}
      data-collapsed={collapsed ? "true" : undefined}
    >
      <header className="cw-panel__header">
        <h2 id={titleId} className="cw-panel__title heading">
          {title}
        </h2>
        {tabs}
        {actions ? <div className="cw-panel__actions">{actions}</div> : null}
      </header>
      {collapsed ? null : <div className="cw-panel__body">{children}</div>}
    </section>
  );
}

/** 1px `border-subtle` rule between regions inside a panel body. */
export function PanelDivider() {
  return <hr className="cw-panel__divider" />;
}
