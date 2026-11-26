// SPDX-License-Identifier: GPL-3.0-or-later
// The export queue panel (WP-2G.04, dockable; View > Panels > Exports): every export of the
// project's queue, first in line first, with its progress, speed and time left. Row actions are
// icon buttons with tooltips: cancel, retry, move up and down (or drag a waiting export onto
// another), reveal the file and take it off the list; a failed export says why. The same actions
// are on the row's context menu (`export.row`). Each row follows its own job in `jobStore`, so a
// progress event re-renders that row only.

import type { ExportQueueJob, JobId, JobInfo } from "@cutweave/protocol";
import {
  IconButton,
  type IconName,
  ProgressBar,
  StatusChip,
  type StatusTone,
  Tooltip,
} from "@cutweave/ui";
import { type DragEvent, memo, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { useContextMenu } from "../menus/host.tsx";
import { EmptyState } from "../panels/EmptyState.tsx";
import { useJobStore } from "../state/jobStore.ts";
import { type Translate, translator } from "../timeline/errors.ts";
import { clockText, flicksToSeconds } from "./exportModel.ts";
import { isFinished, isWaiting, moveTarget, useQueueStore } from "./exportQueue.ts";
import { baseName, percent } from "./exportSettings.ts";
import { exportQueue } from "./useExport.ts";

const STATE_LABEL = {
  queued: "exportQueue.state.queued",
  running: "exportQueue.state.running",
  succeeded: "exportQueue.state.succeeded",
  failed: "exportQueue.state.failed",
  cancelled: "exportQueue.state.cancelled",
} as const satisfies Record<ExportQueueJob["state"], TKey>;

const STATE_TONE: Readonly<Record<ExportQueueJob["state"], StatusTone>> = {
  queued: "neutral",
  running: "neutral",
  succeeded: "success",
  failed: "danger",
  cancelled: "warning",
};

/** What a queue row's menu acts on (`MenuTarget.data`). */
export interface QueueRowData {
  readonly id: JobId;
  readonly path: string;
  readonly state: ExportQueueJob["state"];
  readonly error: string | null;
}

/** The row's speed and time left: "31 fps · about 0:15 left". */
function speedText(progress: JobInfo["progress"], t: Translate): string | null {
  const parts: string[] = [];
  if (progress.fps !== null && progress.fps !== undefined) {
    parts.push(t("export.progress.fps", { fps: String(Math.round(progress.fps * 10) / 10) }));
  }
  if (progress.eta_ms !== null && progress.eta_ms !== undefined) {
    parts.push(t("export.progress.eta", { eta: clockText(progress.eta_ms / 1000) }));
  }
  return parts.length === 0 ? null : parts.join(" · ");
}

/** The MIME type a row's drag carries (a waiting export's id). */
const DRAG_TYPE = "application/x-cutweave-export";

interface RowProps {
  readonly job: ExportQueueJob;
  readonly canUp: boolean;
  readonly canDown: boolean;
  readonly canReveal: boolean;
}

const QueueRow = memo(function QueueRow({ job, canUp, canDown, canReveal }: RowProps) {
  const { t: rawT } = useTranslation();
  const t = translator(rawT);
  const queue = exportQueue();
  // The live job: progress events arrive here, not through the queue list.
  const live = useJobStore((s) => s.jobs[job.id]);
  const state = (live?.state ?? job.state) as ExportQueueJob["state"];
  const progress = live?.progress ?? job.progress;
  const error = live?.error ?? job.error;
  const name = baseName(job.path);
  const pct = state === "succeeded" ? 100 : percent(progress.fraction);
  const [over, setOver] = useState(false);
  const waiting = state === "queued";
  const ref = useContextMenu<HTMLLIElement>("export.row", () => ({
    ids: [job.id],
    name,
    data: { id: job.id, path: job.path, state, error } satisfies QueueRowData,
  }));
  const speed = state === "running" ? speedText(progress, t) : null;
  const drop = (event: DragEvent<HTMLLIElement>) => {
    setOver(false);
    const id = event.dataTransfer.getData(DRAG_TYPE) as JobId;
    if (id === "" || id === job.id || !waiting) return;
    event.preventDefault();
    void queue.reorder(id, job.id);
  };
  const action = (icon: IconName, label: string, run: () => void) => (
    <Tooltip label={label}>
      <IconButton size="sm" icon={icon} label={label} onClick={run} />
    </Tooltip>
  );
  return (
    <li
      ref={ref}
      className="cw-queue__row"
      data-state={state}
      data-job={job.id}
      data-over={over ? "true" : undefined}
      aria-label={t("exportQueue.row", { name, state: t(STATE_LABEL[state]) })}
      draggable={waiting}
      onDragStart={(event) => {
        event.dataTransfer.setData(DRAG_TYPE, job.id);
        event.dataTransfer.effectAllowed = "move";
      }}
      onDragOver={(event) => {
        if (!waiting || !event.dataTransfer.types.includes(DRAG_TYPE)) return;
        event.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={drop}
    >
      <div className="cw-queue__main">
        <div className="cw-queue__line">
          <span className="cw-queue__name body" title={job.path}>
            {name}
          </span>
          {state === "running" ? (
            <span className="cw-queue__pct cw-mono numeric caption">{`${pct}%`}</span>
          ) : (
            <StatusChip tone={STATE_TONE[state]}>{t(STATE_LABEL[state])}</StatusChip>
          )}
        </div>
        {state === "running" || state === "queued" ? (
          <ProgressBar value={pct} label={t("exportQueue.progress", { name })} />
        ) : null}
        <span className="cw-queue__meta caption">
          {[
            t("export.summary.size", { width: String(job.width), height: String(job.height) }),
            clockText(flicksToSeconds(job.duration)),
            speed,
          ]
            .filter((part): part is string => part !== null)
            .join(" · ")}
        </span>
        {state === "failed" && error !== null ? (
          <span className="cw-queue__error caption" role="alert">
            {t("exportQueue.error", { message: error })}
          </span>
        ) : null}
      </div>
      <div className="cw-queue__actions">
        {waiting && canUp
          ? action("arrow-up", t("exportQueue.moveUp"), () => void queue.move(job.id, "up"))
          : null}
        {waiting && canDown
          ? action("arrow-down", t("exportQueue.moveDown"), () => void queue.move(job.id, "down"))
          : null}
        {state === "queued" || state === "running"
          ? action("x", t("exportQueue.cancel"), () => void queue.cancel(job.id))
          : null}
        {state === "failed" || state === "cancelled"
          ? action("refresh-cw", t("exportQueue.retry"), () => void queue.retry(job.id))
          : null}
        {state === "succeeded" && canReveal
          ? action("folder-open", t("exportQueue.reveal"), () => queue.reveal(job.path))
          : null}
        {isFinished({ state })
          ? action("trash-2", t("exportQueue.remove"), () => void queue.remove(job.id))
          : null}
      </div>
    </li>
  );
});

/** The dockable list of the project's exports. */
export function ExportQueuePanel() {
  const { t: rawT } = useTranslation();
  const t = translator(rawT);
  const queue = exportQueue();
  const status = useQueueStore((s) => s.status);
  const paused = useQueueStore((s) => s.paused);
  const jobs = useQueueStore((s) => s.jobs);
  useEffect(() => {
    void queue.refresh();
  }, [queue]);
  const waiting = jobs.filter(isWaiting).length;
  const finished = jobs.some(isFinished);
  const canReveal = queue.canReveal();
  const pauseLabel = paused ? t("exportQueue.resume") : t("exportQueue.pause");
  return (
    <div className="cw-queue" data-testid="export-queue">
      <div className="cw-queue__tools">
        <span className="caption cw-queue__count">
          {paused
            ? t("exportQueue.pausedCount", { count: String(waiting) })
            : t("exportQueue.count", { count: String(waiting) })}
        </span>
        <div className="cw-queue__toolbar">
          <Tooltip label={pauseLabel}>
            <IconButton
              size="sm"
              icon={paused ? "play" : "pause"}
              label={pauseLabel}
              pressed={paused}
              disabled={status !== "ready"}
              onClick={() => void queue.setPaused(!paused)}
            />
          </Tooltip>
          <Tooltip label={t("exportQueue.clearFinished")}>
            <IconButton
              size="sm"
              icon="trash-2"
              label={t("exportQueue.clearFinished")}
              disabled={!finished}
              onClick={() => void queue.clearFinished()}
            />
          </Tooltip>
        </div>
      </div>
      {status === "unavailable" ? (
        <p className="cw-note cw-queue__notice" role="alert">
          {t("exportQueue.unavailable")}
        </p>
      ) : null}
      <div className="cw-queue__scroll">
        {jobs.length === 0 ? (
          <EmptyState
            icon="download"
            title={t("exportQueue.emptyTitle")}
            body={t("exportQueue.emptyBody")}
          />
        ) : (
          <ul className="cw-queue__list" aria-label={t("exportQueue.list")}>
            {jobs.map((job) => (
              <QueueRow
                key={job.id}
                job={job}
                canUp={moveTarget(jobs, job.id, "up") !== undefined}
                canDown={moveTarget(jobs, job.id, "down") !== undefined}
                canReveal={canReveal}
              />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
