// SPDX-License-Identifier: GPL-3.0-or-later
import type { CSSProperties } from "react";
import { cx } from "./cx.ts";

export interface ProgressBarProps {
  /** 0 to 100. */
  value: number;
  /** Names the bar for assistive tech, e.g. the job name. */
  label: string;
  className?: string | undefined;
}

/** Marigold fill on a `border-control` track. Shared by Toast (job) and JobProgress. */
export function ProgressBar({ value, label, className }: ProgressBarProps) {
  const clamped = Math.min(100, Math.max(0, Number.isFinite(value) ? value : 0));
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(clamped)}
      className={cx("cw-bar", className)}
      style={{ "--value": `${clamped}%` } as CSSProperties}
    >
      <div className="cw-bar__fill" />
    </div>
  );
}
