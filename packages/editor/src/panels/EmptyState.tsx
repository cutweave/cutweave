// SPDX-License-Identifier: GPL-3.0-or-later
import { Icon, type IconName } from "@cutweave/ui";
import type { ReactNode } from "react";

export interface EmptyStateProps {
  icon: IconName;
  title: string;
  body: string;
  /** `agent` tints the icon for AI surfaces (design/README.md: agent blue = AI). */
  tone?: "neutral" | "agent";
  children?: ReactNode;
}

/** What will appear here, and the first action. Static copy; no data yet (engine arrives in 1M.02). */
export function EmptyState({ icon, title, body, tone = "neutral", children }: EmptyStateProps) {
  return (
    <div className="cw-empty" data-tone={tone}>
      <Icon name={icon} size="lg" className="cw-empty__icon" />
      <p className="cw-empty__title body-strong">{title}</p>
      <p className="cw-empty__body body">{body}</p>
      {children}
    </div>
  );
}
