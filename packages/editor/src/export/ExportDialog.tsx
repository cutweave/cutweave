// SPDX-License-Identifier: GPL-3.0-or-later
// The export dialog (WP-1M.08, ADR-0018 F14): settings, then the progress of the running export.
// Hiding the dialog does not stop the export: it goes on in the engine, `jobStore` keeps its
// progress and the top bar's chip shows it. The one preset is "MP4 H.264 1080p"; the size, rate
// and quality fields start at what it writes, and changing one makes a custom export.

import {
  Button,
  Dialog,
  DialogNote,
  InspectorRow,
  ProgressBar,
  SegmentedControl,
  Switch,
  TextField,
} from "@cutweave/ui";
import { useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { useDocStore } from "../state/docStore.ts";
import { useJobStore } from "../state/jobStore.ts";
import { type Translate, translator } from "../timeline/errors.ts";
import { exportSource } from "./exportController.ts";
import {
  baseName,
  defaultDraft,
  encoderLabel,
  type FormatDraft,
  type FormatProblem,
  formatProblem,
  fpsText,
  hasMp4Extension,
  hwFor,
  isAbsolutePath,
  parseProgressLine,
  percent,
  presetFor,
  QUALITIES,
} from "./exportSettings.ts";
import { type ExportStore, useExportStore } from "./exportStore.ts";
import { exportController } from "./useExport.ts";

const FORMAT_PROBLEM = {
  width: "export.errors.width",
  height: "export.errors.height",
  fps: "export.errors.fps",
} as const satisfies Record<FormatProblem, TKey>;

const QUALITY_LABEL = {
  low: "export.low",
  medium: "export.medium",
  high: "export.high",
} as const satisfies Record<(typeof QUALITIES)[number], TKey>;

/** What to tell about the hardware encoder under its switch. */
function hardwareNote(probe: ExportStore["probe"], on: boolean, t: Translate): string {
  if (!on) return t("export.hardwareOff");
  if (probe.status === "loading") return t("export.hardwareChecking");
  if (probe.status === "ready") {
    return probe.encoders.length === 0
      ? t("export.hardwareNone")
      : t("export.hardwareFound", { names: probe.encoders.map(encoderLabel).join(", ") });
  }
  return t("export.hardwareAuto");
}

type Stage = "form" | "replace";

function SettingsForm() {
  const { t } = useTranslation();
  const controller = exportController();
  const project = useDocStore((s) => s.project);
  const source = useMemo(() => exportSource(project), [project]);
  const probe = useExportStore((s) => s.probe);
  const [path, setPath] = useState(() => controller.defaultPath());
  // Settings > Export decides where the form starts; it is read once, as the dialog opens.
  const [defaults] = useState(() => controller.formDefaults());
  const [draft, setDraft] = useState<FormatDraft | null>(() =>
    source === null ? null : defaultDraft(source.facts, defaults.quality),
  );
  const [hardware, setHardware] = useState(defaults.hardware);
  const [stage, setStage] = useState<Stage>("form");
  const [submitted, setSubmitted] = useState(false);
  const [starting, setStarting] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const pathId = useId();
  const widthId = useId();
  const heightId = useId();
  const fpsId = useId();
  const hardwareId = useId();

  if (source === null || draft === null) return null;
  const facts = source.facts;
  const problem = formatProblem(draft);
  const pathProblem: TKey | null =
    path.trim() === ""
      ? "export.errors.path"
      : !isAbsolutePath(path)
        ? "export.errors.pathFull"
        : !hasMp4Extension(path)
          ? "export.errors.pathExtension"
          : null;
  const error: TKey | null = pathProblem ?? (problem === null ? null : FORMAT_PROBLEM[problem]);

  const set = (patch: Partial<FormatDraft>): void => setDraft({ ...draft, ...patch });

  const run = async (overwrite: boolean): Promise<void> => {
    setSubmitted(true);
    setRefusal(null);
    if (error !== null) {
      setStage("form");
      return;
    }
    setStarting(true);
    const outcome = await controller.start({
      path: path.trim(),
      preset: presetFor(draft, facts),
      hw: hwFor(hardware),
      overwrite,
    });
    setStarting(false);
    if (outcome.kind === "exists") setStage("replace");
    else if (outcome.kind === "refused") {
      setStage("form");
      setRefusal(outcome.message);
    }
    // "started": the dialog now shows the progress (the active export is set).
  };

  if (stage === "replace") {
    const name = baseName(path);
    return (
      <Dialog
        open
        onOpenChange={(open) => {
          if (!open) controller.close();
        }}
        title={t("export.exists.title", { name })}
        description={t("export.exists.body")}
        footer={
          <>
            <Button variant="ghost" size="lg" onClick={() => setStage("form")}>
              {t("export.exists.back")}
            </Button>
            <Button variant="danger" size="lg" disabled={starting} onClick={() => void run(true)}>
              {t("export.exists.replace")}
            </Button>
          </>
        }
      >
        {null}
      </Dialog>
    );
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) controller.close();
      }}
      onSubmit={() => void run(false)}
      title={t("export.title")}
      description={t("export.description")}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={() => controller.close()}>
            {t("export.cancel")}
          </Button>
          <Button variant="primary" size="lg" disabled={starting} onClick={() => void run(false)}>
            {starting ? t("export.starting") : t("export.start")}
          </Button>
        </>
      }
    >
      <InspectorRow label={t("export.output")} htmlFor={pathId}>
        <TextField
          id={pathId}
          className="cw-export__path"
          value={path}
          onChange={setPath}
          placeholder={t("export.outputPlaceholder")}
          invalid={submitted && pathProblem !== null}
        />
        <Button
          variant="secondary"
          onClick={() => {
            void controller.chooseFile(path).then((picked) => {
              if (picked !== null) setPath(picked);
            });
          }}
        >
          {t("export.choose")}
        </Button>
      </InspectorRow>
      <InspectorRow label={t("export.preset")}>
        <span className="body">{t("export.presetValue")}</span>
      </InspectorRow>
      <InspectorRow label={t("export.width")} htmlFor={widthId}>
        <TextField
          id={widthId}
          numeric
          inputMode="numeric"
          className="cw-export__num"
          value={draft.width}
          onChange={(width) => set({ width })}
          invalid={submitted && problem === "width"}
        />
      </InspectorRow>
      <InspectorRow label={t("export.height")} htmlFor={heightId}>
        <TextField
          id={heightId}
          numeric
          inputMode="numeric"
          className="cw-export__num"
          value={draft.height}
          onChange={(height) => set({ height })}
          invalid={submitted && problem === "height"}
        />
      </InspectorRow>
      <InspectorRow label={t("export.fps")} htmlFor={fpsId}>
        <TextField
          id={fpsId}
          numeric
          inputMode="decimal"
          className="cw-export__num"
          value={draft.fps}
          onChange={(fps) => set({ fps })}
          invalid={submitted && problem === "fps"}
        />
      </InspectorRow>
      <DialogNote>
        {t("export.sequence", {
          width: String(facts.width),
          height: String(facts.height),
          fps: fpsText(facts.fps),
        })}
      </DialogNote>
      <InspectorRow label={t("export.quality")}>
        <SegmentedControl
          aria-label={t("export.qualityLabel")}
          value={draft.quality}
          onChange={(quality) => set({ quality })}
          options={QUALITIES.map((value) => ({ value, label: t(QUALITY_LABEL[value]) }))}
        />
      </InspectorRow>
      <InspectorRow label={t("export.range")}>
        <span className="body">{t("export.rangeAll")}</span>
      </InspectorRow>
      <InspectorRow label={t("export.hardware")} htmlFor={hardwareId}>
        <Switch id={hardwareId} checked={hardware} onCheckedChange={setHardware} />
      </InspectorRow>
      <DialogNote icon="cpu">{hardwareNote(probe, hardware, translator(t))}</DialogNote>
      {submitted && error !== null ? (
        <p className="cw-note" role="alert">
          {t(error)}
        </p>
      ) : refusal !== null ? (
        <p className="cw-note" role="alert">
          {refusal}
        </p>
      ) : null}
    </Dialog>
  );
}

/** The running export: percent, speed and time left, with Hide and Cancel. */
function ProgressView() {
  const { t } = useTranslation();
  const controller = exportController();
  const active = useExportStore((s) => s.active);
  const cancelling = useExportStore((s) => s.cancelling);
  const info = useJobStore((s) => (active === null ? undefined : s.jobs[active.jobId]));
  if (active === null) return null;
  const pct = percent(info?.progress.fraction ?? null);
  const line = parseProgressLine(info?.progress.message ?? null);
  const speed = [
    line.fps === null ? null : t("export.progress.fps", { fps: line.fps }),
    line.eta === null ? null : t("export.progress.eta", { eta: line.eta }),
  ].filter((part): part is string => part !== null);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) controller.close();
      }}
      title={t("export.progress.title", { name: active.fileName })}
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
      {line.restartedInSoftware ? (
        <DialogNote icon="triangle-alert">{t("export.progress.restarted")}</DialogNote>
      ) : null}
      <DialogNote>{t("export.progress.hint")}</DialogNote>
    </Dialog>
  );
}

/** The export dialog; mount once in the shell. */
export function ExportDialog() {
  const open = useExportStore((s) => s.dialogOpen);
  const running = useExportStore((s) => s.active !== null);
  if (!open) return null;
  return running ? <ProgressView /> : <SettingsForm />;
}
