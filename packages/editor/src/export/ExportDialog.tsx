// SPDX-License-Identifier: GPL-3.0-or-later
// The export dialog (WP-1M.08, WP-2G.04, ADR-0018 F14, F34): the settings (`ExportForm`), then the
// progress of the export Export started. Hiding the dialog does not stop the export: it goes on
// in the engine, `jobStore` keeps its progress and the top bar's chip shows it.

import type { JobInfo } from "@cutweave/protocol";
import { Button, Dialog, DialogNote, ProgressBar } from "@cutweave/ui";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useDocStore } from "../state/docStore.ts";
import { useJobStore } from "../state/jobStore.ts";
import { translator } from "../timeline/errors.ts";
import { ExportForm } from "./ExportForm.tsx";
import { exportSource } from "./exportController.ts";
import { humanBytes } from "./exportModel.ts";
import { parseProgressLine, percent } from "./exportSettings.ts";
import { type ActiveExport, useExportStore } from "./exportStore.ts";
import { exportController } from "./useExport.ts";

/** The progress of every file of the active export, 0 to 1 (finished files count as done). */
export function activeFraction(
  active: ActiveExport,
  jobs: Readonly<Record<string, JobInfo>>,
): number | null {
  if (active.files.length === 0) return null;
  let sum = 0;
  let known = false;
  for (const file of active.files) {
    const info = jobs[file.jobId];
    const f = info?.state === "succeeded" ? 1 : (info?.progress.fraction ?? null);
    if (f !== null) known = true;
    sum += f ?? 0;
  }
  return known ? sum / active.files.length : null;
}

/** The job of the active export that is running now (else the first). */
function currentJob(
  active: ActiveExport,
  jobs: Readonly<Record<string, JobInfo>>,
): JobInfo | undefined {
  const infos = active.files.map((f) => jobs[f.jobId]);
  return infos.find((info) => info?.state === "running") ?? infos[0];
}

/** "Exporting Film.mp4", or "Exporting 4 files". */
export function activeName(active: ActiveExport, files: (count: number) => string): string {
  return active.files.length === 1 ? (active.files[0]?.fileName ?? "") : files(active.files.length);
}

/** The running export: percent, speed and time left, with Hide and Cancel. */
function ProgressView() {
  const { t: rawT } = useTranslation();
  const t = translator(rawT);
  const controller = exportController();
  const active = useExportStore((s) => s.active);
  const cancelling = useExportStore((s) => s.cancelling);
  const jobs = useJobStore((s) => s.jobs);
  if (active === null) return null;
  const pct = percent(activeFraction(active, jobs));
  const info = currentJob(active, jobs);
  const line = parseProgressLine(info?.progress.message ?? null);
  const speed = [
    line.fps === null ? null : t("export.progress.fps", { fps: line.fps }),
    line.eta === null ? null : t("export.progress.eta", { eta: line.eta }),
  ].filter((part): part is string => part !== null);
  const name = activeName(active, (count) => t("export.files", { count: String(count) }));
  const software = active.warnings.some((w) => w.code === "SoftwareEncoder");
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) controller.close();
      }}
      title={t("export.progress.title", { name })}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={() => controller.close()}>
            {t("export.progress.hide")}
          </Button>
          <Button
            variant="secondary"
            size="lg"
            disabled={cancelling}
            onClick={() => void controller.cancel()}
          >
            {cancelling ? t("export.progress.cancelling") : t("export.progress.cancel")}
          </Button>
        </>
      }
    >
      <div className="cw-export__progress" data-testid="export-progress">
        <ProgressBar value={pct} label={t("export.progress.label")} />
        <div className="cw-export__stats">
          <span className="cw-mono numeric body-strong">{`${pct}%`}</span>
          <span className="caption cw-export__speed">
            {speed.length === 0 ? t("export.progress.starting") : speed.join(" · ")}
          </span>
        </div>
      </div>
      {active.estimatedKib !== null ? (
        <DialogNote icon="file">
          {t("export.progress.estimate", { size: humanBytes(active.estimatedKib * 1024) })}
        </DialogNote>
      ) : null}
      {software ? <DialogNote icon="cpu">{t("export.warnings.software")}</DialogNote> : null}
      {line.restartedInSoftware ? (
        <DialogNote icon="triangle-alert">{t("export.progress.restarted")}</DialogNote>
      ) : null}
      <DialogNote>{t("export.progress.hint")}</DialogNote>
    </Dialog>
  );
}

/** The settings, once there is a sequence to export. */
function SettingsView() {
  const project = useDocStore((s) => s.project);
  const source = useMemo(() => exportSource(project), [project]);
  if (source === null) return null;
  return <ExportForm source={source} />;
}

/** The export dialog; mount once in the shell. */
export function ExportDialog() {
  const open = useExportStore((s) => s.dialogOpen);
  const running = useExportStore((s) => s.active !== null);
  if (!open) return null;
  return running ? <ProgressView /> : <SettingsView />;
}
