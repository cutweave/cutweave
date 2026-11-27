// SPDX-License-Identifier: GPL-3.0-or-later
// "Archive project…" (WP-2F.04b): bundle the project, its media and a checked file list
// (`archive.json`) into one folder or one zip file, to store or hand over. Choose the name, the
// location and the format; the dialog says how much it is and whether it fits, then shows the
// job's progress and where the archive is. The OS folder picker is the only native part.
import {
  Button,
  Dialog,
  DialogNote,
  InspectorRow,
  JobList,
  JobProgress,
  SegmentedControl,
  Switch,
  TextField,
} from "@cutweave/ui";
import { useEffect, useId } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { engineStore } from "../engine/engineStore.ts";
import { desktopFromWindow } from "../session/desktop.ts";
import { projectController } from "../session/useProject.ts";
import { formatBytes } from "../settings/cacheStatus.ts";
import { useJobStore } from "../state/jobStore.ts";
import {
  archiveDestOf,
  archiveStore,
  cancelArchive,
  estimateArchive,
  startArchive,
} from "./archive.ts";
import { phaseOf, validFolderName } from "./collect.ts";
import { openRelinkDialog } from "./RelinkDialog.tsx";

/** Opens the dialog, proposing "<project> archive" as the name. */
export function openArchiveDialog(): void {
  archiveStore.getState().show(`${projectController().projectName()} archive`);
}

const deps = { engine: engineStore };

export function ArchiveDialog() {
  const { t } = useTranslation();
  const state = useStore(archiveStore);
  const job = useJobStore((s) => (state.jobId === null ? undefined : s.jobs[state.jobId]));
  const nameId = useId();
  const parentId = useId();
  const trimId = useId();
  const dest = archiveDestOf(state);
  const phase = phaseOf(state.jobId, job);

  // Ask for the estimate whenever the form names a place (again when it changes).
  useEffect(() => {
    if (state.open && phase === "form" && dest !== null) void estimateArchive(deps);
  }, [state.open, phase, dest, state.trimUnused]);

  if (!state.open) return null;
  const close = () => archiveStore.getState().close();
  const estimate = state.estimate;
  const missing = estimate?.missing.length ?? 0;
  const ready =
    dest !== null &&
    estimate !== null &&
    missing === 0 &&
    estimate.enough_space &&
    state.error === null;
  const start = () => {
    if (ready) void startArchive(deps);
  };
  const again = () => {
    archiveStore.getState().set({ jobId: null, error: null });
  };

  if (phase !== "form") {
    const zip = state.madeFormat === "zip";
    const percent = phase === "done" ? 100 : Math.round((job?.progress.fraction ?? 0) * 100);
    return (
      <Dialog
        open
        onOpenChange={(next) => {
          // A running archive keeps going in the background.
          if (!next) close();
        }}
        title={t(phase === "done" ? "archive.doneTitle" : "archive.runningTitle")}
        description={
          phase === "done"
            ? t(zip ? "archive.doneBodyZip" : "archive.doneBodyFolder", { path: state.dest ?? "" })
            : phase === "cancelled"
              ? t(zip ? "archive.cancelledBodyZip" : "archive.cancelledBodyFolder")
              : undefined
        }
        footer={
          <>
            <Button variant="ghost" size="lg" onClick={close}>
              {t(phase === "running" ? "collect.hide" : "collect.close")}
            </Button>
            {phase === "done" ? (
              <Button
                variant="primary"
                size="lg"
                icon="folder-open"
                onClick={() => void desktopFromWindow()?.shell?.showItemInFolder(state.dest ?? "")}
              >
                {t(zip ? "archive.revealFile" : "archive.revealFolder")}
              </Button>
            ) : phase === "running" ? null : (
              <Button variant="primary" size="lg" onClick={again}>
                {t(zip ? "archive.again" : "collect.continue")}
              </Button>
            )}
          </>
        }
      >
        <JobList aria-label={t("archive.jobLabel")}>
          <JobProgress
            name={t("archive.jobName", { name: state.name.trim() })}
            state={
              phase === "done"
                ? "done"
                : phase === "running"
                  ? job?.state === "running"
                    ? "running"
                    : "queued"
                  : "failed"
            }
            progress={percent}
            error={phase === "failed" ? (job?.error ?? undefined) : undefined}
            onCancel={phase === "running" ? () => void cancelArchive(deps) : undefined}
          />
        </JobList>
      </Dialog>
    );
  }

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close();
      }}
      onSubmit={start}
      title={t("archive.title")}
      description={t("archive.hint")}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("collect.cancel")}
          </Button>
          <Button variant="primary" size="lg" disabled={!ready} onClick={start}>
            {t("archive.start")}
          </Button>
        </>
      }
    >
      <InspectorRow label={t("archive.name")} htmlFor={nameId}>
        <TextField
          id={nameId}
          value={state.name}
          onChange={(name) => archiveStore.getState().edit({ name })}
          invalid={!validFolderName(state.name)}
          autoFocus
        />
      </InspectorRow>
      <InspectorRow label={t("collect.location")} htmlFor={parentId}>
        <TextField
          id={parentId}
          value={state.parent}
          onChange={(parent) => archiveStore.getState().edit({ parent })}
          placeholder={t("archive.locationNone")}
        />
        <Button
          variant="secondary"
          onClick={() => {
            void projectController()
              .chooseFolder()
              .then((picked) => {
                if (picked !== null) archiveStore.getState().edit({ parent: picked });
              });
          }}
        >
          {t("collect.choose")}
        </Button>
      </InspectorRow>
      <InspectorRow label={t("archive.format")}>
        <SegmentedControl
          aria-label={t("archive.format")}
          value={state.format}
          onChange={(format) => archiveStore.getState().edit({ format })}
          options={[
            { value: "folder", label: t("archive.formatFolder") },
            { value: "zip", label: t("archive.formatZip") },
          ]}
        />
      </InspectorRow>
      <InspectorRow label={t("collect.trimUnused")} htmlFor={trimId}>
        <Switch
          id={trimId}
          checked={state.trimUnused}
          onCheckedChange={(trimUnused) => archiveStore.getState().edit({ trimUnused })}
        />
      </InspectorRow>
      {state.error !== null ? (
        <p className="cw-note" role="alert">
          {state.error}
        </p>
      ) : estimate === null ? (
        <DialogNote>{t(dest === null ? "archive.needPlace" : "collect.estimating")}</DialogNote>
      ) : missing > 0 ? (
        <p className="cw-note" role="alert">
          {missing === 1
            ? t("collect.missingOne", { name: estimate.missing[0]?.name ?? "" })
            : t("collect.missingMany", { count: String(missing) })}{" "}
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              close();
              openRelinkDialog();
            }}
          >
            {t("contextMenu.item.relink")}
          </Button>
        </p>
      ) : !estimate.enough_space ? (
        <p className="cw-note" role="alert">
          {t("collect.noSpace", {
            size: formatBytes(estimate.bytes),
            free: formatBytes(estimate.free_bytes ?? 0),
          })}
        </p>
      ) : (
        <DialogNote>
          <span role="status">
            {t(estimate.files === 1 ? "collect.estimateOne" : "collect.estimate", {
              count: String(estimate.files),
              size: formatBytes(estimate.bytes),
            })}
            {estimate.free_bytes === null
              ? ""
              : ` ${t("collect.free", { free: formatBytes(estimate.free_bytes) })}`}
          </span>
        </DialogNote>
      )}
    </Dialog>
  );
}
