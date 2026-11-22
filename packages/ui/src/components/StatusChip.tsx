// SPDX-License-Identifier: GPL-3.0-or-later
import type { ReactNode } from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";

export type StatusTone = "success" | "warning" | "danger" | "neutral";

const TONE_ICON: Record<StatusTone, IconName> = {
  success: "check",
  warning: "triangle-alert",
  danger: "circle-alert",
  neutral: "info",
};

export interface StatusChipProps {
  tone: StatusTone;
  /** One to three words, sentence case: "Exported", "Missing media". */
  children: ReactNode;
  className?: string | undefined;
}

/** Status as icon plus word, never colour alone. Static: it takes no focus and has no hover. */
export function StatusChip({ tone, children, className }: StatusChipProps) {
  return (
    <span className={cx("cw-status label", className)} data-tone={tone}>
      <Icon name={TONE_ICON[tone]} size="sm" />
      {children}
    </span>
  );
}
