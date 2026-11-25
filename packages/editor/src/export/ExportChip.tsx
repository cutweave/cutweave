// SPDX-License-Identifier: GPL-3.0-or-later
// The top bar's export status (WP-1M.08): shown while an export runs, as an icon and the percent,
// so it stays small in a bar that is already tight. The tooltip has the details; pressing it
// brings the progress dialog back.

import { Button, Tooltip } from "@cutweave/ui";
import { useTranslation } from "react-i18next";
import { ContextSurface } from "../menus/host.tsx";
import { useJobStore } from "../state/jobStore.ts";
import { percent } from "./exportSettings.ts";
import { useExportStore } from "./exportStore.ts";
import { exportController } from "./useExport.ts";

export function ExportStatusChip() {
  const { t } = useTranslation();
  const active = useExportStore((s) => s.active);
  const info = useJobStore((s) => (active === null ? undefined : s.jobs[active.jobId]));
  if (active === null) return null;
  const pct = String(percent(info?.progress.fraction ?? null));
  return (
    <ContextSurface surface="chrome.export" className="cw-context-surface">
      <Tooltip label={t("export.chip.tooltip", { name: active.fileName, percent: pct })}>
        <Button
          variant="ghost"
          size="sm"
          icon="loader-circle"
          className="cw-export-chip"
          data-testid="export-chip"
          onClick={() => exportController().open()}
        >
          {t("export.chip.label", { percent: pct })}
        </Button>
      </Tooltip>
    </ContextSurface>
  );
}
