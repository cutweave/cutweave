// SPDX-License-Identifier: GPL-3.0-or-later
import type { HTMLAttributes } from "react";
import { Icon } from "../icons/index.ts";
import { Button } from "./Button.tsx";
import { cx } from "./cx.ts";
import { IconButton } from "./IconButton.tsx";
import { ProgressBar } from "./ProgressBar.tsx";
import { StatusChip } from "./StatusChip.tsx";

export type JobState = "running" | "queued" | "failed" | "done";

export interface JobProgressProps {
  /** Real asset and model names: "Exporting Shorts_final.mp4". */
  name: string;
  state: JobState;
  /** 0 to 100. */
  progress: number;
  /** "20 s left", "Waiting". */
  eta?: string | undefined;
  /** Failure message; says what happened and the next step. */
  error?: string | undefined;
  /** Shown while running or queued. Dispatches `job.cancel`. */
  onCancel?: (() => void) | undefined;
  /** Shown when failed. Dispatches `job.retry`. */
  onRetry?: (() => void) | undefined;
}

/** `role="list"` container for JobProgress rows. */
export function JobList({ className, ...rest }: HTMLAttributes<HTMLUListElement>) {
  return <ul className={cx("cw-joblist", className)} {...rest} />;
}

/** One background job: name, bar, percent, ETA and cancel (or retry when failed). */
export function JobProgress({
  name,
  state,
  progress,
  eta,
  error,
  onCancel,
  onRetry,
}: JobProgressProps) {
  const pct = `${Math.round(Math.min(100, Math.max(0, progress)))}%`;
  return (
    <li className="cw-job" data-state={state}>
      <div className="cw-job__main">
        <span className="cw-job__name body">{name}</span>
        {state === "queued" ? (
          <StatusChip tone="neutral" className="cw-job__chip">
            Queued
          </StatusChip>
        ) : state === "done" ? (
          <StatusChip tone="success" className="cw-job__chip">
            Done
          </StatusChip>
        ) : (
          <ProgressBar value={progress} label={name} />
        )}
        {state === "failed" && error ? (
          <span className="cw-job__err caption" role="alert">
            <Icon name="circle-alert" size="sm" />
            {error}
          </span>
        ) : null}
      </div>
      <span className="cw-job__pct cw-mono numeric">{pct}</span>
      {eta ? <span className="cw-job__eta caption">{eta}</span> : null}
      {state === "failed" ? (
        <Button size="sm" onClick={onRetry} aria-label={`Retry ${name}`}>
          Retry
        </Button>
      ) : state === "done" ? null : (
        <IconButton size="sm" icon="x" label={`Cancel ${name}`} onClick={onCancel} />
      )}
    </li>
  );
}
