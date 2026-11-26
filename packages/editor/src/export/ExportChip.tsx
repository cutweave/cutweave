// SPDX-License-Identifier: GPL-3.0-or-later
// The top bar's export status (WP-1M.08): shown while an export runs, as an icon and the percent,
// so it stays small in a bar that is already tight. The tooltip has the details; pressing it
// brings the progress dialog back.

import { Button, Tooltip } from "@cutweave/ui";
import { useTranslation } from "react-i18next";
import { ContextSurface } from "../menus/host.tsx";
import { useJobStore } from "../state/jobStore.ts";
import { translator } from "../timeline/errors.ts";
import { activeFraction, activeName } from "./ExportDialog.tsx";
import { percent } from "./exportSettings.ts";
import { useExportStore } from "./exportStore.ts";
import { exportController } from "./useExport.ts";

export function ExportStatusChip() {
  const { t: rawT } = useTranslation();
  const t = translator(rawT);
  const active = useExportStore((s) => s.active);
  const pct = useJobStore((s) => (active === null ? 0 : percent(activeFraction(active, s.jobs))));
  if (active === null) return null;
  const name = activeName(active, (count) => t("export.files", { count: String(count) }));
  return (
    <ContextSurface surface="chrome.export" className="cw-context-surface">
      <Tooltip label={t("export.chip.tooltip", { name, percent: String(pct) })}>
        <Button
          variant="ghost"
          size="sm"
          icon="loader-circle"
          className="cw-export-chip"
          data-testid="export-chip"
          onClick={() => exportController().open()}
        >
          {t("export.chip.label", { percent: String(pct) })}
        </Button>
      </Tooltip>
    </ContextSurface>
  );
}
