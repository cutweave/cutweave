// SPDX-License-Identifier: GPL-3.0-or-later
// "Collect files…" (WP-2F.04): copy the project and all its media into one folder that opens on
// any computer. Choose where and under which name; the dialog says how much it is and whether it
// fits, then shows the copy's progress. Cancelling keeps what was copied: Collect again continues.
// The OS folder picker is the only native part.
import {
  Button,
  Dialog,
  DialogNote,
  InspectorRow,
  JobList,
  JobProgress,
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
  cancelCollect,
  collectStore,
  destOf,
  estimateCollect,
  phaseOf,
  startCollect,
  validFolderName,
} from "./collect.ts";
import { openRelinkDialog } from "./RelinkDialog.tsx";

/** Opens the dialog, proposing "<project> collected" as the folder name. */
export function openCollectDialog(): void {
  collectStore.getState().show(`${projectController().projectName()} collected`);
}

const deps = { engine: engineStore };

export function CollectDialog() {
  const { t } = useTranslation();
  const state = useStore(collectStore);
  const job = useJobStore((s) => (state.jobId === null ? undefined : s.jobs[state.jobId]));
  const nameId = useId();
  const parentId = useId();
  const trimId = useId();
  const dest = destOf(state);
  const phase = phaseOf(state.jobId, job);

  // Ask for the estimate whenever the form names a folder (again when it changes).
  useEffect(() => {
    if (state.open && phase === "form" && dest !== null) void estimateCollect(deps);
  }, [state.open, phase, dest, state.trimUnused]);

  if (!state.open) return null;
  const close = () => collectStore.getState().close();
  const estimate = state.estimate;
  const missing = estimate?.missing.length ?? 0;
  const ready =
    dest !== null &&
    estimate !== null &&
    missing === 0 &&
    estimate.enough_space &&
    state.error === null;
  const start = () => {
    if (ready) void startCollect(deps);
  };
  const again = () => {
    collectStore.getState().set({ jobId: null, error: null });
  };

  if (phase !== "form") {
    const percent = phase === "done" ? 100 : Math.round((job?.progress.fraction ?? 0) * 100);
    return (
      <Dialog
        open
        onOpenChange={(next) => {
          // A running copy keeps going in the background.
          if (!next) close();
        }}
        title={t(phase === "done" ? "collect.doneTitle" : "collect.runningTitle")}
        description={
          phase === "done"
            ? t("collect.doneBody", { path: state.dest ?? "" })
            : phase === "cancelled"
              ? t("collect.cancelledBody")
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
                {t("collect.reveal")}
              </Button>
            ) : phase === "running" ? null : (
              <Button variant="primary" size="lg" onClick={again}>
                {t("collect.continue")}
              </Button>
            )}
          </>
        }
      >
        <JobList aria-label={t("collect.jobLabel")}>
          <JobProgress
            name={t("collect.jobName", { name: state.name.trim() })}
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
            onCancel={phase === "running" ? () => void cancelCollect(deps) : undefined}
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
      title={t("collect.title")}
      description={t("collect.hint")}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("collect.cancel")}
          </Button>
          <Button variant="primary" size="lg" disabled={!ready} onClick={start}>
            {t("collect.start")}
          </Button>
        </>
      }
    >
      <InspectorRow label={t("collect.name")} htmlFor={nameId}>
        <TextField
          id={nameId}
          value={state.name}
          onChange={(name) => collectStore.getState().edit({ name })}
          invalid={!validFolderName(state.name)}
          autoFocus
        />
      </InspectorRow>
      <InspectorRow label={t("collect.location")} htmlFor={parentId}>
        <TextField
          id={parentId}
          value={state.parent}
          onChange={(parent) => collectStore.getState().edit({ parent })}
          placeholder={t("collect.locationNone")}
        />
        <Button
          variant="secondary"
          onClick={() => {
            void projectController()
              .chooseFolder()
              .then((picked) => {
                if (picked !== null) collectStore.getState().edit({ parent: picked });
              });
          }}
        >
          {t("collect.choose")}
        </Button>
      </InspectorRow>
      <InspectorRow label={t("collect.trimUnused")} htmlFor={trimId}>
        <Switch
          id={trimId}
          checked={state.trimUnused}
          onCheckedChange={(trimUnused) => collectStore.getState().edit({ trimUnused })}
        />
      </InspectorRow>
      {state.error !== null ? (
        <p className="cw-note" role="alert">
          {state.error}
        </p>
      ) : estimate === null ? (
        <DialogNote>{t(dest === null ? "collect.needFolder" : "collect.estimating")}</DialogNote>
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
