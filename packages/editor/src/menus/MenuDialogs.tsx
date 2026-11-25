// SPDX-License-Identifier: GPL-3.0-or-later
// The questions context-menu choices ask (WP-2H.13, `dialogs.ts`): rename a clip, confirm deleting
// a track with clips on it, confirm removing a media asset clips use. Each sends exactly one
// command when the person agrees, through the same call every menu choice uses.
import { Button, Dialog, TextField } from "@cutweave/ui";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { engineStore } from "../engine/engineStore.ts";
import { removeMedia } from "../media/remove.ts";
import { toastStore } from "../state/toastStore.ts";
import { translator } from "../timeline/errors.ts";
import { menuServices } from "./defaults.ts";
import { menuDialogStore, useMenuDialog } from "./dialogs.ts";

function RenameClip({ item, name }: { readonly item: string; readonly name: string }) {
  const { t } = useTranslation();
  const [value, setValue] = useState(name);
  useEffect(() => setValue(name), [name]);
  const close = () => menuDialogStore.getState().close();
  const submit = () => {
    close();
    if (value.trim() === name) return;
    void menuServices
      .call("item.set_props", { item, props: { name: value.trim() } })
      .catch((error: unknown) => menuServices.report(error));
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("contextMenu.dialog.renameClipTitle")}
      onSubmit={submit}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("contextMenu.dialog.cancel")}
          </Button>
          <Button variant="primary" size="lg" onClick={submit}>
            {t("contextMenu.dialog.rename")}
          </Button>
        </>
      }
    >
      <div className="cw-row">
        <TextField
          aria-label={t("contextMenu.dialog.clipName")}
          value={value}
          onChange={setValue}
          autoFocus
          onFocus={(event) => event.currentTarget.select()}
        />
      </div>
    </Dialog>
  );
}

function DeleteTrack({
  track,
  name,
  clips,
}: {
  readonly track: string;
  readonly name: string;
  readonly clips: number;
}) {
  const { t } = useTranslation();
  const close = () => menuDialogStore.getState().close();
  const confirm = () => {
    close();
    void menuServices.call("track.remove", { track }).catch((error: unknown) => {
      menuServices.report(error);
    });
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("contextMenu.dialog.deleteTrackTitle", { name })}
      description={t("contextMenu.dialog.deleteTrackBody", { count: clips })}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("contextMenu.dialog.cancel")}
          </Button>
          <Button variant="danger" size="lg" onClick={confirm}>
            {t("contextMenu.dialog.deleteTrack")}
          </Button>
        </>
      }
    >
      <p className="body">{t("contextMenu.dialog.deleteTrackNote")}</p>
    </Dialog>
  );
}

function RemoveMedia({
  id,
  name,
  uses,
}: {
  readonly id: string;
  readonly name: string;
  readonly uses: number;
}) {
  const { t } = useTranslation();
  const close = () => menuDialogStore.getState().close();
  const confirm = () => {
    close();
    void removeMedia(
      { engine: engineStore, toasts: toastStore, t: translator(t) },
      id as Parameters<typeof removeMedia>[1],
      name,
    );
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("mediaBin.remove.confirmTitle", { name })}
      description={t("mediaBin.remove.confirmBody", { count: String(uses) })}
      footer={
        <>
          <Button variant="ghost" size="lg" onClick={close}>
            {t("mediaBin.remove.cancel")}
          </Button>
          <Button variant="danger" size="lg" onClick={confirm}>
            {t("mediaBin.remove.confirm")}
          </Button>
        </>
      }
    >
      <p className="body">{t("mediaBin.remove.confirmNote")}</p>
    </Dialog>
  );
}

/** Mounted once in the shell, next to the menu host. */
export function MenuDialogs() {
  const dialog = useMenuDialog();
  if (dialog === null) return null;
  switch (dialog.kind) {
    case "rename-clip":
      return <RenameClip key={dialog.item} item={dialog.item} name={dialog.name} />;
    case "delete-track":
      return (
        <DeleteTrack
          key={dialog.track}
          track={dialog.track}
          name={dialog.name}
          clips={dialog.clips}
        />
      );
    case "remove-media":
      return <RemoveMedia key={dialog.id} id={dialog.id} name={dialog.name} uses={dialog.uses} />;
  }
}
