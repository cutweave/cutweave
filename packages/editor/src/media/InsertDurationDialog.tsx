// SPDX-License-Identifier: GPL-3.0-or-later
// "Duration…" on an image card (#215; ADR-0035, Decision 1): how long this picture is the next
// time it goes onto the timeline. Nothing is sent: the choice is kept for this run of the app
// (`insertDuration.ts`) and the next insert of the card carries it.
import { Button, Dialog, DialogNote, TextField } from "@cutweave/ui";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { menuDialogStore } from "../menus/dialogs.ts";
import { useDocStore } from "../state/docStore.ts";
import { flicksToSeconds, insertDurationStore, secondsToFlicks } from "./insertDuration.ts";
import { proposedSeconds } from "./slideshow.ts";

export function InsertDurationDialog({
  media,
  name,
}: {
  readonly media: string;
  readonly name: string;
}) {
  const { t } = useTranslation();
  const project = useDocStore((s) => s.project);
  const chosen = insertDurationStore.getState().byMedia.get(media);
  const [seconds, setSeconds] = useState(() =>
    chosen === undefined
      ? proposedSeconds(project?.settings?.still_duration ?? null)
      : flicksToSeconds(chosen),
  );
  const length = secondsToFlicks(seconds);
  const close = () => menuDialogStore.getState().close();
  const submit = () => {
    if (length === null) return;
    insertDurationStore.getState().set(media, length);
    close();
  };
  const useDefault = () => {
    insertDurationStore.getState().set(media, null);
    close();
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("contextMenu.dialog.insertDurationTitle", { name })}
      description={t("contextMenu.dialog.insertDurationBody")}
      onSubmit={submit}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("contextMenu.dialog.cancel")}
          </Button>
          {chosen !== undefined && (
            <Button variant="secondary" size="lg" onClick={useDefault}>
              {t("contextMenu.dialog.insertDurationDefault")}
            </Button>
          )}
          <Button variant="primary" size="lg" disabled={length === null} onClick={submit}>
            {t("contextMenu.dialog.insertDurationSet")}
          </Button>
        </>
      }
    >
      <div className="cw-row">
        <label className="body" htmlFor="insert-duration-seconds">
          {t("contextMenu.dialog.insertDurationSeconds")}
        </label>
        <TextField
          id="insert-duration-seconds"
          numeric
          inputMode="decimal"
          value={seconds}
          onChange={setSeconds}
          invalid={length === null}
          autoFocus
          onFocus={(event) => event.currentTarget.select()}
        />
      </div>
      <DialogNote icon="info">{t("contextMenu.dialog.insertDurationNote")}</DialogNote>
    </Dialog>
  );
}
