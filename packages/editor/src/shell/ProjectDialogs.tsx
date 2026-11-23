// SPDX-License-Identifier: GPL-3.0-or-later
// The dialogs of the project flows (WP-1M.07): crash recovery (Restore / Discard), "Save
// changes?" before something that would lose them, the name and place of a new or copied
// project, and the banner for a project from a newer version (read-only).

import { Button, Dialog, DialogNote, InspectorRow, TextField } from "@cutweave/ui";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { clockTime } from "../session/errors.ts";
import { type SessionStore, useSessionStore } from "../session/sessionStore.ts";
import {
  type NameProblem,
  type NamingPrompt,
  projectNameProblem,
  useSessionUiStore,
} from "../session/sessionUiStore.ts";
import { projectController } from "../session/useProject.ts";

const NAME_PROBLEM: Record<NameProblem, TKey> = {
  empty: "project.new.errors.nameEmpty",
  invalid: "project.new.errors.nameInvalid",
  long: "project.new.errors.nameLong",
};

/** Restore or discard the unsaved work a crashed session left behind. Not dismissable. */
function RecoveryDialog() {
  const { t } = useTranslation();
  const recovery = useSessionStore((s: SessionStore) => s.recovery);
  const busy = useSessionUiStore((s) => s.busy);
  const controller = projectController();
  if (recovery === null) return null;
  const time = clockTime(recovery.last_ts);
  const projectName = controller.projectName();
  const count = String(recovery.records);
  return (
    <Dialog
      open
      onOpenChange={() => {}}
      dirty
      title={t("project.recovery.title")}
      footer={
        <>
          <Button
            variant="ghost"
            size="lg"
            disabled={busy}
            onClick={() => void controller.discardRecovery()}
          >
            {t("project.recovery.discard")}
          </Button>
          <Button
            variant="primary"
            size="lg"
            disabled={busy}
            onClick={() => void controller.recover()}
          >
            {t("project.recovery.restoreCount", { count, time })}
          </Button>
        </>
      }
    >
      <p className="body">
        {recovery.records === 1
          ? t("project.recovery.bodyOne", { name: projectName, time })
          : t("project.recovery.body", { name: projectName, count, time })}
      </p>
      {recovery.damaged ? (
        <DialogNote icon="triangle-alert">{t("project.recovery.damaged")}</DialogNote>
      ) : null}
      <DialogNote>{t("project.recovery.note")}</DialogNote>
    </Dialog>
  );
}

/** Save, Discard or Cancel before leaving a project with unsaved changes. */
function UnsavedDialog() {
  const { t } = useTranslation();
  const prompt = useSessionUiStore((s) => s.unsaved);
  if (prompt === null) return null;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) prompt.resolve("cancel");
      }}
      title={t("project.unsaved.title", { name: prompt.project })}
      description={t("project.unsaved.body")}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={() => prompt.resolve("cancel")}>
            {t("project.unsaved.cancel")}
          </Button>
          <Button variant="danger" size="lg" onClick={() => prompt.resolve("discard")}>
            {t("project.unsaved.discard")}
          </Button>
          <Button variant="primary" size="lg" onClick={() => prompt.resolve("save")}>
            {t("project.unsaved.save")}
          </Button>
        </>
      }
    >
      {null}
    </Dialog>
  );
}

function NamingForm({ prompt }: { prompt: NamingPrompt }) {
  const { t } = useTranslation();
  const controller = projectController();
  const nameId = useId();
  const parentId = useId();
  const [name, setName] = useState(prompt.initialName);
  const [parent, setParent] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const problem = projectNameProblem(name);
  const missingParent = parent.trim() === "";
  const error: TKey | null =
    problem !== null
      ? NAME_PROBLEM[problem]
      : missingParent
        ? "project.new.errors.locationMissing"
        : null;
  const isNew = prompt.mode === "new";

  const submit = (): void => {
    setSubmitted(true);
    if (error !== null) return;
    prompt.resolve(controller.target(name, parent.trim()));
  };

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) prompt.resolve(null);
      }}
      onSubmit={submit}
      title={t(isNew ? "project.new.title" : "project.new.saveAsTitle")}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={() => prompt.resolve(null)}>
            {t("project.new.cancel")}
          </Button>
          <Button variant="primary" size="lg" onClick={submit}>
            {t(isNew ? "project.new.create" : "project.new.save")}
          </Button>
        </>
      }
    >
      <InspectorRow label={t("project.new.name")} htmlFor={nameId}>
        <TextField
          id={nameId}
          value={name}
          onChange={setName}
          placeholder={t("project.new.namePlaceholder")}
          invalid={submitted && problem !== null}
          // The first field takes the focus when the dialog opens.
          autoFocus
        />
      </InspectorRow>
      <InspectorRow label={t("project.new.location")} htmlFor={parentId}>
        <TextField
          id={parentId}
          value={parent}
          onChange={setParent}
          placeholder={t("project.new.locationNone")}
          invalid={submitted && missingParent}
        />
        <Button
          variant="secondary"
          onClick={() => {
            void controller.chooseFolder().then((picked) => {
              if (picked !== null) setParent(picked);
            });
          }}
        >
          {t("project.new.choose")}
        </Button>
      </InspectorRow>
      {submitted && error !== null ? (
        <p className="cw-note" role="alert">
          {t(error)}
        </p>
      ) : (
        <DialogNote>{t("project.new.hint", { name: name.trim() || "…" })}</DialogNote>
      )}
    </Dialog>
  );
}

function NamingDialog() {
  const prompt = useSessionUiStore((s) => s.naming);
  return prompt === null ? null : <NamingForm key={prompt.mode} prompt={prompt} />;
}

/** All project dialogs; mount once in the shell. */
export function ProjectDialogs() {
  return (
    <>
      <RecoveryDialog />
      <UnsavedDialog />
      <NamingDialog />
    </>
  );
}

/** A project from a newer version opens read-only: say so, and offer the way out. */
export function ReadOnlyBanner() {
  const { t } = useTranslation();
  const readOnly = useSessionStore((s) => s.readOnly);
  const controller = projectController();
  if (readOnly === null) return null;
  return (
    <div className="cw-banner" role="status">
      <span className="cw-banner__text">
        <span className="body-strong">{t("project.readOnlyTitle")}</span>
        <span className="body">{t("project.readOnlyBody")}</span>
      </span>
      <Button variant="secondary" onClick={() => void controller.saveAs()}>
        {t("topbar.saveAs")}
      </Button>
    </div>
  );
}
